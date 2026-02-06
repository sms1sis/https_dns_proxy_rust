use std::net::{SocketAddr, IpAddr};
use anyhow::{Result, Context};
use tokio::net::{UdpSocket, TcpListener};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use reqwest::{Client, Url};
use std::sync::{Arc, Mutex};
use tokio::sync::{RwLock, mpsc};

use std::time::{Duration, Instant};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::collections::{VecDeque, HashMap};
use std::sync::LazyLock;
use bytes::Bytes;
use moka::future::Cache;
use jni::JavaVM;
use jni::objects::JClass;
use hickory_resolver::proto::op::Message;

pub struct Stats {
    pub queries_udp: AtomicUsize,
    pub queries_tcp: AtomicUsize,
    pub errors: AtomicUsize,
}

struct LogMessage {
    domain: String,
    status: String,
}

static QUERY_LOGS: LazyLock<Mutex<VecDeque<String>>> = LazyLock::new(|| Mutex::new(VecDeque::with_capacity(50))));
static LOG_SENDER: LazyLock<mpsc::UnboundedSender<LogMessage>> = LazyLock::new(|| {
    let (tx, mut rx) = mpsc::unbounded_channel::<LogMessage>();
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let mut logs = QUERY_LOGS.lock().unwrap();
            if logs.len() >= 50 {
                logs.pop_front();
            }
            let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
            logs.push_back(format!("[{}] {} -> {}", timestamp, msg.domain, msg.status));
        }
    });
    tx
});

struct NativeLog {
    level: String,
    msg: String,
}

static NATIVE_LOG_SENDER: LazyLock<mpsc::UnboundedSender<NativeLog>> = LazyLock::new(|| {
    let (tx, mut rx) = mpsc::unbounded_channel::<NativeLog>();
    
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            while let Some(log) = rx.recv().await {
                match log.level.as_str() {
                    "ERROR" => log::error!(target: "SafeDNS-Native", "{}", log.msg),
                    "WARN" => log::warn!(target: "SafeDNS-Native", "{}", log.msg),
                    "INFO" => log::info!(target: "SafeDNS-Native", "{}", log.msg),
                    _ => log::debug!(target: "SafeDNS-Native", "{}", log.msg),
                }

                if let Ok(jvm_lock) = JVM.read() {
                    if let Some(jvm) = jvm_lock.as_ref() {
                        if let Ok(class_lock) = PROXY_SERVICE_CLASS.read() {
                            if let Some(class_ref) = class_lock.as_ref() {
                                if let Ok(mut env) = jvm.attach_current_thread() {
                                    if let Ok(level_j) = env.new_string(&log.level) {
                                        if let Ok(tag_j) = env.new_string("SafeDNS-Native") {
                                            if let Ok(msg_j) = env.new_string(&log.msg) {
                                                let _ = env.call_static_method(
                                                    class_ref,
                                                    "nativeLog",
                                                    "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V",
                                                    &[(&level_j).into(), (&tag_j).into(), (&msg_j).into()],
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
    });
    tx
});

fn native_log(level: &str, msg: &str) {
    if !cfg!(debug_assertions) {
        match level {
            "ERROR" | "WARN" => {}
            _ => return,
        }
    }
    let _ = NATIVE_LOG_SENDER.send(NativeLog {
        level: level.to_string(),
        msg: msg.to_string(),
    });
}

#[cfg(feature = "jni")]
static GLOBAL_CACHE: LazyLock<RwLock<Option<DnsCache>>> = LazyLock::new(|| RwLock::new(None));

static LAST_LATENCY: AtomicUsize = AtomicUsize::new(0);

static JVM: LazyLock<std::sync::RwLock<Option<JavaVM>>> = LazyLock::new(|| std::sync::RwLock::new(None));
static PROXY_SERVICE_CLASS: LazyLock<std::sync::RwLock<Option<jni::objects::GlobalRef>>> = LazyLock::new(|| std::sync::RwLock::new(None));

fn add_query_log(domain: String, status: String) {
    let _ = LOG_SENDER.send(LogMessage { domain, status });
}
        Self {
            queries_udp: AtomicUsize::new(0),
            queries_tcp: AtomicUsize::new(0),
            errors: AtomicUsize::new(0),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub listen_addr: String,
    pub listen_port: u16,
    pub tcp_client_limit: usize,
    pub bootstrap_dns: String,
    pub polling_interval: u64,
    pub force_ipv4: bool,
    pub allow_ipv6: bool,
    pub resolver_url: String, 
    pub proxy_server: Option<String>,
    pub source_addr: Option<String>,
    pub http11: bool,
    pub http3: bool,
    pub max_idle_time: u64,
    pub conn_loss_time: u64,
    pub ca_path: Option<String>,
    pub statistic_interval: u64,
    pub cache_ttl: u64,
    pub exclude_domain: Option<String>,
}

type DnsCache = Cache<Bytes, (Bytes, Instant)>;

#[derive(Clone)]
struct DynamicResolver {
    hosts: Arc<RwLock<HashMap<String, Vec<SocketAddr>>>>,
}

impl DynamicResolver {
    fn new() -> Self {
        Self {
            hosts: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn update(&self, domain: String, addrs: Vec<SocketAddr>) {
        let mut hosts = self.hosts.write().await;
        hosts.insert(domain, addrs);
    }
}

impl Resolve for DynamicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let name_str = name.as_str().to_string();
        let hosts = self.hosts.clone();
        Box::pin(async move {
            let hosts = hosts.read().await;
            if let Some(addrs) = hosts.get(&name_str) {
                native_log("DEBUG", &format!("DynamicResolver: {} -> {:?}", name_str, addrs));
                Ok(Box::new(addrs.clone().into_iter()) as Addrs)
            } else {
                Err(Box::new(std::io::Error::new(std::io::ErrorKind::NotFound, format!("Host {} not found in DynamicResolver", name_str))) as Box<dyn std::error::Error + Send + Sync>)
            }
        })
    }
}

pub async fn run_proxy(config: Config, stats: Arc<Stats>, mut shutdown_rx: tokio::sync::oneshot::Receiver<()>) -> Result<()> {
    let addr: SocketAddr = format!("{}:{}", config.listen_addr, config.listen_port)
        .parse()
        .context("Failed to parse listen address")?;

    let resolver_url_parsed = Url::parse(&config.resolver_url)
        .context("Failed to parse resolver URL")?;
    let resolver_domain = resolver_url_parsed.domain().context("Resolver URL must have a domain")?.to_string();

    // Retry binding to handle transient port conflicts during restarts
    let mut udp_socket = None;
    let mut tcp_listener = None;
    for i in 0..5 {
        let bind_result = (|| {
            let udp_sock = socket2::Socket::new(
                if addr.is_ipv4() { socket2::Domain::IPV4 } else { socket2::Domain::IPV6 },
                socket2::Type::DGRAM,
                Some(socket2::Protocol::UDP),
            )?;
            udp_sock.set_reuse_address(true)?;
            #[cfg(unix)]
            udp_sock.set_reuse_port(true)?;
            udp_sock.bind(&addr.into())?;
            let udp_tokio = UdpSocket::from_std(udp_sock.into())?;

            let tcp_sock = socket2::Socket::new(
                if addr.is_ipv4() { socket2::Domain::IPV4 } else { socket2::Domain::IPV6 },
                socket2::Type::STREAM,
                Some(socket2::Protocol::TCP),
            )?;
            tcp_sock.set_reuse_address(true)?;
            #[cfg(unix)]
            tcp_sock.set_reuse_port(true)?;
            tcp_sock.bind(&addr.into())?;
            tcp_sock.listen(128)?;
            let tcp_tokio = TcpListener::from_std(tcp_sock.into())?;

            Ok::<(UdpSocket, TcpListener), anyhow::Error>((udp_tokio, tcp_tokio))
        })();

        match bind_result {
            Ok((u, t)) => {
                udp_socket = Some(Arc::new(u));
                tcp_listener = Some(t);
                break;
            }
            Err(e) => {
                native_log("ERROR", &format!("Failed to bind sockets (attempt {}): {}", i + 1, e));
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let udp_socket = udp_socket.context("Failed to bind UDP socket after retries")?;
    let tcp_listener = tcp_listener.context("Failed to bind TCP listener after retries")?;

    native_log("INFO", &format!("Listening on UDP/TCP {} -> {}", addr, config.resolver_url));

    let ips = resolve_bootstrap(&resolver_domain, &config.bootstrap_dns, config.allow_ipv6).await?;
    native_log("INFO", &format!("Bootstrapped {} to {:?}", resolver_domain, ips));
    
    let dynamic_resolver = DynamicResolver::new();
    dynamic_resolver.update(resolver_domain.clone(), ips).await;

    let client = create_client(&config, dynamic_resolver.clone())?;
    let resolver_url_str = Arc::new(config.resolver_url.clone());
    
    // DNS Cache: 2048 entries
    let cache: DnsCache = Cache::builder()
        .max_capacity(2048)
        .build();

    #[cfg(feature = "jni")]
    {
        let mut w = GLOBAL_CACHE.write().await;
        *w = Some(cache.clone());
    }

    // Bootstrap Refresh Loop (updates DynamicResolver instead of recreating Client)
    let bootstrap_handle = {
        let dynamic_resolver = dynamic_resolver.clone();
        let config = config.clone();
        let domain = resolver_domain.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(config.polling_interval)));
            loop {
                interval.tick().await;
                match resolve_bootstrap(&domain, &config.bootstrap_dns, config.allow_ipv6).await {
                    Ok(new_ips) => {
                        native_log("DEBUG", &format!("Refreshed bootstrap IPs for {}: {:?}", domain, new_ips));
                        dynamic_resolver.update(domain.clone(), new_ips).await;
                    }
                    Err(e) => native_log("ERROR", &format!("Failed to refresh bootstrap IP: {}", e)),
                }
            }
        })
    };

    let tcp_semaphore = Arc::new(Semaphore::new(config.tcp_client_limit)));

    let mut udp_loop = {
        let socket = udp_socket.clone();
        let client = client.clone();
        let resolver_url = resolver_url_str.clone();
        let stats = stats.clone();
        let cache = cache.clone();
        let cache_ttl = config.cache_ttl;
        let exclude_domain = config.exclude_domain.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                match socket.recv_from(&mut buf).await {
                    Ok((len, peer)) => {
                        let data = Bytes::copy_from_slice(&buf[..len]);
                        let socket = socket.clone();
                        let client = client.clone();
                        let resolver_url = resolver_url.clone();
                        let stats = stats.clone();
                        let cache = cache.clone();
                        let exclude_domain = exclude_domain.clone();
                        tokio::spawn(async move {
                            stats.queries_udp.fetch_add(1, Ordering::Relaxed);
                            if let Err(e) = handle_udp_query(socket, client, resolver_url, data, peer, stats, cache, cache_ttl, exclude_domain).await {
                                native_log("DEBUG", &format!("UDP error from {}: {:#}", peer, e));
                            }
                        });
                    }
                    Err(e) => native_log("ERROR", &format!("UDP recv error: {}", e)),
                }
            }
        })
    };

    let mut tcp_loop = {
        let client = client.clone();
        let resolver_url = resolver_url_str.clone();
        let semaphore = tcp_semaphore.clone();
        let stats = stats.clone();
        let cache = cache.clone();
        let cache_ttl = config.cache_ttl;
        let exclude_domain = config.exclude_domain.clone();
        tokio::spawn(async move {
            loop {
                match tcp_listener.accept().await {
                    Ok((mut stream, peer)) => {
                        let client = client.clone();
                        let resolver_url = resolver_url.clone();
                        let permit = semaphore.clone().acquire_owned().await;
                        let stats = stats.clone();
                        let cache = cache.clone();
                        let exclude_domain = exclude_domain.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            stats.queries_tcp.fetch_add(1, Ordering::Relaxed);
                            if let Err(e) = handle_tcp_query(&mut stream, client, resolver_url, stats, cache, cache_ttl, exclude_domain).await {
                                native_log("DEBUG", &format!("TCP error from {}: {}", peer, e));
                            }
                        });
                    }
                    Err(e) => native_log("ERROR", &format!("TCP accept error: {}", e)),
                }
            }
        })
    };

    tokio::select! {
        _ = &mut shutdown_rx => native_log("INFO", &format!("Shutting down proxy...")),
        _ = &mut udp_loop => native_log("ERROR", &format!("UDP loop exited unexpectedly")),
        _ = &mut tcp_loop => native_log("ERROR", &format!("TCP loop exited unexpectedly")),
    }

    udp_loop.abort();
    tcp_loop.abort();
    bootstrap_handle.abort();
    Ok(())
}

#[cfg(feature = "jni")]
pub mod jni_api {
    use super::*;
    use jni::JNIEnv;
    use jni::objects::{JClass, JObject, JString};
    use jni::sys::jint;
    use tokio::runtime::Runtime;
    use tokio_util::sync::CancellationToken;

    static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| Runtime::new().unwrap()));
    static CANCELLATION_TOKEN: LazyLock<Mutex<Option<CancellationToken>>> = LazyLock::new(|| Mutex::new(None));

    #[unsafe(no_mangle)]
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_io_github_SafeDNS_ProxyService_initLogger(
        mut env: JNIEnv,
        _class: JClass,
        _context: JObject,
    ) {
         let filter = if cfg!(debug_assertions) {
             log::LevelFilter::Debug
         } else {
             log::LevelFilter::Info
         };

         android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(filter)
                .with_tag("SafeDNS")
         );
         
         if let Ok(jvm) = env.get_java_vm() {
             if let Ok(mut w) = JVM.write() {
                 *w = Some(jvm);
             }
         }

         if let Ok(class) = env.find_class("io/github/SafeDNS/ProxyService") {
             if let Ok(global_ref) = env.new_global_ref(class) {
                 if let Ok(mut w) = PROXY_SERVICE_CLASS.write() {
                     *w = Some(global_ref);
                 }
             }
         }

         #[cfg(target_os = "android")]
         rustls_platform_verifier::android::init_hosted(&mut env, _context).ok();
         native_log("INFO", "Logger, JVM and Global Class Ref initialized");
    }
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_io_github_SafeDNS_ProxyService_startProxy(
        mut env: JNIEnv,
        _class: JClass,
        listen_addr: JString,
        listen_port: jint,
        resolver_url: JString,
        bootstrap_dns: JString,
        allow_ipv6: jni::sys::jboolean,
        cache_ttl: jni::sys::jlong,
        tcp_limit: jint,
        poll_interval: jni::sys::jlong,
        use_http3: jni::sys::jboolean,
        exclude_domain: JString,
    ) -> jint {
        let listen_addr: String = env.get_string(&listen_addr).unwrap().into();
        let resolver_url: String = env.get_string(&resolver_url).unwrap().into();
        let bootstrap_dns: String = env.get_string(&bootstrap_dns).unwrap().into();
        let exclude_domain: String = env.get_string(&exclude_domain).unwrap().into();
        let allow_ipv6 = allow_ipv6 != 0;
        let use_http3 = use_http3 != 0;

        native_log("INFO", &format!("startProxy: addr={}, port={}, resolver={}", listen_addr, listen_port, resolver_url));

        let config = Config {
            listen_addr,
            listen_port: listen_port as u16,
            resolver_url,
            bootstrap_dns,
            allow_ipv6,
            tcp_client_limit: tcp_limit as usize,
            polling_interval: poll_interval as u64,
            force_ipv4: !allow_ipv6,
            proxy_server: None,
            source_addr: None,
            http11: false,
            http3: use_http3,
            max_idle_time: 120,
            conn_loss_time: 10,
            ca_path: None,
            statistic_interval: 0,
            cache_ttl: cache_ttl as u64,
            exclude_domain: if exclude_domain.is_empty() { None } else { Some(exclude_domain) },
        };

        let token = CancellationToken::new();
        let cloned_token = token.clone();
        {
            let mut lock = CANCELLATION_TOKEN.lock().unwrap();
            *lock = Some(token);
        }

        let stats = Arc::new(Stats::new());
        RUNTIME.block_on(async {
            let mut w = GLOBAL_STATS.write().await;
            *w = Some(stats.clone());
        });

        let config_clone = config.clone();
        let stats_clone = stats.clone();
        
        RUNTIME.spawn(async move {
            let (tx, rx) = tokio::sync::oneshot::channel();
            
            tokio::spawn(async move {
                cloned_token.cancelled().await;
                let _ = tx.send(());
            });

            if let Err(e) = run_proxy(config_clone, stats_clone, rx).await {
                native_log("ERROR", &format!("Proxy error: {}", e));
            }
        });

        0
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_io_github_SafeDNS_ProxyService_getLatency(
        _env: JNIEnv,
        _class: JClass,
    ) -> jint {
        let lat = LAST_LATENCY.load(Ordering::Relaxed) as jint;
        if lat > 0 {
            native_log("DEBUG", &format!("JNI getLatency: {}ms", lat));
        }
        lat
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_io_github_SafeDNS_ProxyService_getLogs(
        mut env: JNIEnv,
        _class: JClass,
    ) -> jni::sys::jobjectArray {
        let logs = QUERY_LOGS.lock().unwrap();
        let list: Vec<String> = logs.iter().cloned().collect();
        
        let cls = env.find_class("java/lang/String").unwrap();
        let initial = env.new_string("").unwrap();
        let array = env.new_object_array(list.len() as jni::sys::jsize, cls, &initial).unwrap();
        
        for (i, log) in list.iter().enumerate() {
            let s = env.new_string(log).unwrap();
            env.set_object_array_element(&array, i as jni::sys::jsize, &s).unwrap();
        }
        
        array.into_raw()
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_io_github_SafeDNS_ProxyService_clearLogs(
        _env: JNIEnv,
        _class: JClass,
    ) {
        let mut logs = QUERY_LOGS.lock().unwrap();
        logs.clear();
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_io_github_SafeDNS_ProxyService_stopProxy(
        _env: JNIEnv,
        _class: JClass,
    ) {
        let mut lock = CANCELLATION_TOKEN.lock().unwrap();
        if let Some(token) = lock.take() {
            token.cancel();
        }
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_io_github_SafeDNS_ProxyService_clearCache(
        _env: JNIEnv,
        _class: JClass,
    ) {
        RUNTIME.spawn(async {
            if let Some(cache) = &*GLOBAL_CACHE.read().await {
                cache.invalidate_all();
                native_log("DEBUG", &format!("DNS Cache cleared via JNI"));
            }
        });
    }
}

async fn resolve_bootstrap(domain: &str, bootstrap_dns: &str, allow_ipv6: bool) -> Result<Vec<SocketAddr>> {
    let servers: Vec<SocketAddr> = bootstrap_dns
        .split(',')
        .map(|s| {
            let s = s.trim();
            if let Ok(ip) = s.parse::<IpAddr>() {
                SocketAddr::new(ip, 53)
            } else {
                s.parse().expect("Invalid bootstrap address")
            }
        })
        .collect();

    let mut config = ResolverConfig::new();
    for s in servers {
        config.add_name_server(NameServerConfig::new(s, Protocol::Udp));
        config.add_name_server(NameServerConfig::new(s, Protocol::Tcp));
    }

    let mut opts = ResolverOpts::default();
    opts.ip_strategy = if allow_ipv6 {
        hickory_resolver::config::LookupIpStrategy::Ipv4thenIpv6
    } else {
        hickory_resolver::config::LookupIpStrategy::Ipv4Only
    };

    let resolver = TokioResolver::builder_with_config(config, TokioConnectionProvider::default())
        .with_options(opts)
        .build();
    let ips = resolver.lookup_ip(domain).await.context("Failed to resolve DoH provider")?;
    
    let addrs: Vec<SocketAddr> = ips.iter().map(|ip| SocketAddr::new(ip, 443)).collect();
    
    if addrs.is_empty() {
        return Err(anyhow::anyhow!("No IPs found for {}", domain));
    }
    
    Ok(addrs)
}

fn create_client(config: &Config, resolver: DynamicResolver) -> Result<Client> {
    let mut builder = Client::builder()
        .user_agent("SafeDNS/0.4.0")
        .dns_resolver(Arc::new(resolver))
        .tls_backend_rustls()
        .tcp_nodelay(true)
        .pool_idle_timeout(Duration::from_secs(30)) // Shorter idle timeout for mobile reliability
        .pool_max_idle_per_host(8)
        .connect_timeout(Duration::from_secs(10));

    if config.http11 { 
        builder = builder.http1_only(); 
    } else {
        // Standard negotiation (H2/H3) is more reliable than prior_knowledge
        builder = builder.http2_adaptive_window(true);
    }

    if let Some(proxy_url) = &config.proxy_server {
        builder = builder.proxy(Proxy::all(proxy_url)?);
    }

    if let Some(source_addr) = &config.source_addr {
        let ip = source_addr.parse::<IpAddr>()?;
        builder = builder.local_address(ip);
    }

    if let Some(ca_path) = &config.ca_path {
        let mut buf = Vec::new();
        File::open(ca_path)?.read_to_end(&mut buf)?;
        builder = builder.add_root_certificate(reqwest::Certificate::from_pem(&buf)?);
    }

    Ok(builder.no_proxy().build()?)
}

async fn handle_udp_query(
    socket: Arc<UdpSocket>,
    client: Client,
    resolver_url: Arc<String>,
    data: Bytes,
    peer: SocketAddr,
    stats: Arc<Stats>,
    cache: DnsCache,
    cache_ttl_default: u64,
    exclude_domain: Option<String>,
) -> Result<()> {
    match forward_to_doh(client, resolver_url, data, stats.clone(), cache, cache_ttl_default, exclude_domain).await {
        Ok(bytes) => {
            socket.send_to(&bytes, peer).await?;
            Ok(())
        }
        Err(e) => {
            stats.errors.fetch_add(1, Ordering::Relaxed);
            native_log("DEBUG", &format!("UDP error from {}: {:#}", peer, e));
            Err(e)
        }
    }
}

async fn handle_tcp_query(
    stream: &mut tokio::net::TcpStream,
    client: Client,
    resolver_url: Arc<String>,
    stats: Arc<Stats>,
    cache: DnsCache,
    cache_ttl_default: u64,
    exclude_domain: Option<String>,
) -> Result<()> {
    let mut len_buf = [0u8; 2];
    stream.read_exact(&mut len_buf).await?;
    let len = u16::from_be_bytes(len_buf) as usize;
    
    let mut data = vec![0u8; len];
    stream.read_exact(&mut data).await?;
    let data = Bytes::from(data);

    match forward_to_doh(client, resolver_url, data, stats.clone(), cache, cache_ttl_default, exclude_domain).await {
        Ok(bytes) => {
            let resp_len = (bytes.len() as u16).to_be_bytes();
            stream.write_all(&resp_len).await?;
            stream.write_all(&bytes).await?;
            Ok(())
        }
        Err(e) => {
            stats.errors.fetch_add(1, Ordering::Relaxed);
            Err(e)
        }
    }
}

fn extract_domain(data: &[u8]) -> String {
    if let Ok(msg) = Message::from_vec(data) {
        if let Some(query) = msg.queries().first() {
            let name = query.name().to_string();
            return if name.ends_with('.') && name.len() > 1 {
                name[..name.len() - 1].to_string()
            } else {
                name
            };
        }
    }

    if data.len() <= 12 { return "unknown".to_string(); }
    let mut d = String::new();
    let mut i = 12;
    while i < data.len() && data[i] != 0 {
        let len = data[i] as usize;
        i += 1;
        if i + len > data.len() { break; }
        if !d.is_empty() { d.push('.'); }
        d.push_str(&String::from_utf8_lossy(&data[i..i+len]));
        i += len;
    }
    if d.is_empty() { "unknown".to_string() } else { d }
}

async fn forward_to_doh(
    client: Client,
    resolver_url: Arc<String>,
    data: Bytes,
    _stats: Arc<Stats>,
    cache: DnsCache,
    cache_ttl_default: u64,
    exclude_domain: Option<String>,
) -> Result<Bytes> {
    if data.len() < 12 {
        return Err(anyhow::anyhow!("DNS message too short"));
    }

    let original_id = [data[0], data[1]];
    let domain = extract_domain(&data);
    let should_cache = if let Some(ref exclude) = exclude_domain {
        !domain.eq_ignore_ascii_case(exclude)
    } else {
        true
    };
    
    // 1. Check Cache
    if should_cache {
        let cache_key = data.slice(2..);
        if let Some((cached_resp, expiry)) = cache.get(&cache_key).await {
            if Instant::now() < expiry {
                let remaining = expiry.duration_since(Instant::now()).as_secs();
                let mut resp = vec![0u8; cached_resp.len()];
                resp.copy_from_slice(&cached_resp);
                // Restore original ID
                resp[0] = original_id[0];
                resp[1] = original_id[1];
                
                add_query_log(domain, format!("OK (Cache, TTL {})", remaining));
                return Ok(Bytes::from(resp));
            } else {
                cache.invalidate(&cache_key).await;
            }
        }
    }

    // RFC 8484: The DNS message ID MUST be 0 in every DNS request.
    let mut request_data = data.to_vec();
    request_data[0] = 0;
    request_data[1] = 0;

    let start = std::time::Instant::now();

    // Implement retries for robustness
    let mut last_err = None;
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(100 * attempt as u64)).await;
        }
        let resp = client
            .post(&*resolver_url)
            .header("content-type", "application/dns-message")
            .header("accept", "application/dns-message")
            .body(request_data.clone())
            .send()
            .await;

        match resp {
            Ok(r) => {
                let version = r.version();
                if !r.status().is_success() {
                    last_err = Some(anyhow::anyhow!("Resolver status {} (v{:?})", r.status(), version));
                    continue;
                }
                let bytes = r.bytes().await?;
                let latency = start.elapsed().as_millis() as usize;
                LAST_LATENCY.store(latency, Ordering::Relaxed);
                add_query_log(domain.clone(), format!("OK ({}ms, att {})", latency, attempt + 1));
                
                // 2. Update Cache with TTL extraction
                if should_cache && bytes.len() > 2 {
                    let cache_key = data.slice(2..);
                    let mut ttl = cache_ttl_default; // Default TTL from config
                    if let Ok(msg) = Message::from_vec(&bytes) {
                        ttl = msg.answers().iter().map(|a| a.ttl()).min().unwrap_or(cache_ttl_default as u32).into();
                        if ttl < 10 { ttl = 10; }
                        if ttl > 3600 { ttl = 3600; }
                    }
                    let expiry = Instant::now() + Duration::from_secs(ttl);
                    cache.insert(cache_key.clone(), (bytes.clone(), expiry)).await;
                }

                // Restore original ID in the response
                let mut final_resp = bytes.to_vec();
                if final_resp.len() >= 2 {
                    final_resp[0] = original_id[0];
                    final_resp[1] = original_id[1];
                }

                return Ok(Bytes::from(final_resp));
            }
            Err(e) => {
                last_err = Some(e.into());
            }
        }
    }

    let err_msg = if let Some(e) = last_err.as_ref() {
        let mut msg = e.to_string();
        if msg.contains("connection closed") || msg.contains("broken pipe") {
            msg = format!("Conn Closed: {}", msg);
        } else if msg.contains("timed out") {
            msg = format!("Timeout: {}", msg);
        }
        msg
    } else {
        "Unknown Error".to_string()
    };

    add_query_log(domain, format!("Error: {}", err_msg)));
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Unknown error")))
}

