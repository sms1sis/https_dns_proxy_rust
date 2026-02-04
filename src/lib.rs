use std::net::{SocketAddr, IpAddr};
use anyhow::{Result, Context};
use tokio::net::{UdpSocket, TcpListener};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use reqwest::{Client, Url, Proxy};
use reqwest::dns::{Resolve, Resolving, Name, Addrs};
use std::sync::{Arc, Mutex};
use tokio::sync::{RwLock, Semaphore, mpsc};
use tracing::{info, error, debug};
use hickory_resolver::config::{ResolverConfig, NameServerConfig, ResolverOpts};
use hickory_resolver::proto::xfer::Protocol;
use hickory_resolver::proto::op::Message;
use hickory_resolver::TokioResolver;
use hickory_resolver::name_server::TokioConnectionProvider;
use std::time::Duration;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::fs::File;
use std::io::Read;
use std::collections::{VecDeque, HashMap};
use std::sync::LazyLock;
use bytes::Bytes;
use moka::future::Cache;

pub struct Stats {
    pub queries_udp: AtomicUsize,
    pub queries_tcp: AtomicUsize,
    pub errors: AtomicUsize,
}

struct LogMessage {
    domain: String,
    status: String,
}

static QUERY_LOGS: LazyLock<Mutex<VecDeque<String>>> = LazyLock::new(|| Mutex::new(VecDeque::with_capacity(50)));
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

#[cfg(feature = "jni")]
static GLOBAL_STATS: LazyLock<RwLock<Option<Arc<Stats>>>> = LazyLock::new(|| RwLock::new(None));
#[cfg(feature = "jni")]
static GLOBAL_CACHE: LazyLock<RwLock<Option<DnsCache>>> = LazyLock::new(|| RwLock::new(None));

static LAST_LATENCY: AtomicUsize = AtomicUsize::new(0);

fn add_query_log(domain: String, status: String) {
    debug!("QUERY: {} -> {}", domain, status);
    let _ = LOG_SENDER.send(LogMessage { domain, status });
}

impl Stats {
    pub fn new() -> Self {
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
}

type DnsCache = Cache<Bytes, (Bytes, u32)>;

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

    async fn update(&self, domain: String, addr: SocketAddr) {
        let mut hosts = self.hosts.write().await;
        hosts.insert(domain, vec![addr]);
    }
}

impl Resolve for DynamicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let name_str = name.as_str().to_string();
        let hosts = self.hosts.clone();
        Box::pin(async move {
            let hosts = hosts.read().await;
            if let Some(addrs) = hosts.get(&name_str) {
                Ok(Box::new(addrs.clone().into_iter()) as Addrs)
            } else {
                Err(Box::new(std::io::Error::new(std::io::ErrorKind::NotFound, "Host not found in DynamicResolver")) as Box<dyn std::error::Error + Send + Sync>)
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
        match UdpSocket::bind(addr).await {
            Ok(s) => {
                match TcpListener::bind(addr).await {
                    Ok(l) => {
                        udp_socket = Some(Arc::new(s));
                        tcp_listener = Some(l);
                        break;
                    }
                    Err(e) => {
                        error!("Failed to bind TCP listener (attempt {}): {}", i + 1, e);
                    }
                }
            }
            Err(e) => {
                error!("Failed to bind UDP socket (attempt {}): {}", i + 1, e);
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let udp_socket = udp_socket.context("Failed to bind UDP socket after retries")?;
    let tcp_listener = tcp_listener.context("Failed to bind TCP listener after retries")?;

    info!("Listening on UDP/TCP {} -> {}", addr, config.resolver_url);

    let ip = resolve_bootstrap(&resolver_domain, &config.bootstrap_dns, config.allow_ipv6).await?;
    info!("Bootstrapped {} to {}", resolver_domain, ip);
    
    let dynamic_resolver = DynamicResolver::new();
    dynamic_resolver.update(resolver_domain.clone(), ip).await;

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
            let mut interval = tokio::time::interval(Duration::from_secs(config.polling_interval));
            loop {
                interval.tick().await;
                match resolve_bootstrap(&domain, &config.bootstrap_dns, config.allow_ipv6).await {
                    Ok(new_ip) => {
                        debug!("Refreshed bootstrap IP for {}: {}", domain, new_ip);
                        dynamic_resolver.update(domain.clone(), new_ip).await;
                    }
                    Err(e) => error!("Failed to refresh bootstrap IP: {}", e),
                }
            }
        })
    };

    let tcp_semaphore = Arc::new(Semaphore::new(config.tcp_client_limit));

    let udp_loop = {
        let socket = udp_socket.clone();
        let client = client.clone();
        let resolver_url = resolver_url_str.clone();
        let stats = stats.clone();
        let cache = cache.clone();
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
                        tokio::spawn(async move {
                            stats.queries_udp.fetch_add(1, Ordering::Relaxed);
                            if let Err(e) = handle_udp_query(socket, client, resolver_url, data, peer, stats, cache).await {
                                debug!("UDP error from {}: {:#}", peer, e);
                            }
                        });
                    }
                    Err(e) => error!("UDP recv error: {}", e),
                }
            }
        })
    };

    let tcp_loop = {
        let client = client.clone();
        let resolver_url = resolver_url_str.clone();
        let semaphore = tcp_semaphore.clone();
        let stats = stats.clone();
        let cache = cache.clone();
        tokio::spawn(async move {
            loop {
                match tcp_listener.accept().await {
                    Ok((mut stream, peer)) => {
                        let client = client.clone();
                        let resolver_url = resolver_url.clone();
                        let permit = semaphore.clone().acquire_owned().await;
                        let stats = stats.clone();
                        let cache = cache.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            stats.queries_tcp.fetch_add(1, Ordering::Relaxed);
                            if let Err(e) = handle_tcp_query(&mut stream, client, resolver_url, stats, cache).await {
                                debug!("TCP error from {}: {}", peer, e);
                            }
                        });
                    }
                    Err(e) => error!("TCP accept error: {}", e),
                }
            }
        })
    };

    tokio::select! {
        _ = &mut shutdown_rx => info!("Shutting down proxy..."),
        _ = udp_loop => error!("UDP loop exited unexpectedly"),
        _ = tcp_loop => error!("TCP loop exited unexpectedly"),
    }

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

    static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| Runtime::new().unwrap());
    static CANCELLATION_TOKEN: LazyLock<Mutex<Option<CancellationToken>>> = LazyLock::new(|| Mutex::new(None));

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_io_github_SafeDNS_ProxyService_initLogger(
        mut _env: JNIEnv,
        _class: JClass,
        _context: JObject,
    ) {
        #[cfg(target_os = "android")]
        {
            use log::LevelFilter;
            let level = if cfg!(debug_assertions) {
                LevelFilter::Debug
            } else {
                LevelFilter::Info
            };

            android_logger::init_once(
                android_logger::Config::default()
                    .with_max_level(level)
                    .with_tag("https_dns_proxy"),
            );

            match rustls_platform_verifier::android::init_hosted(&mut _env, _context) {
                Ok(_) => log::info!("Platform verifier initialized successfully"),
                Err(e) => log::error!("Failed to init platform verifier: {:?}", e),
            }
        }
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
    ) -> jint {
        let listen_addr: String = env.get_string(&listen_addr).unwrap().into();
        let resolver_url: String = env.get_string(&resolver_url).unwrap().into();
        let bootstrap_dns: String = env.get_string(&bootstrap_dns).unwrap().into();
        let allow_ipv6 = allow_ipv6 != 0;

        let config = Config {
            listen_addr,
            listen_port: listen_port as u16,
            tcp_client_limit: 20,
            bootstrap_dns,
            polling_interval: 120,
            force_ipv4: !allow_ipv6,
            allow_ipv6,
            resolver_url,
            proxy_server: None,
            source_addr: None,
            http11: false,
            http3: false,
            max_idle_time: 118,
            conn_loss_time: 15,
            ca_path: None,
            statistic_interval: 0,
            cache_ttl: cache_ttl as u64,
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
                error!("Proxy error: {}", e);
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
            debug!("JNI getLatency: {}ms", lat);
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
                debug!("DNS Cache cleared via JNI");
            }
        });
    }
}

async fn resolve_bootstrap(domain: &str, bootstrap_dns: &str, allow_ipv6: bool) -> Result<SocketAddr> {
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
    
    let ip = ips.iter().find(|ip| ip.is_ipv4()).or_else(|| ips.iter().find(|ip| ip.is_ipv6())).ok_or_else(|| anyhow::anyhow!("No IPs found"))?;
    
    Ok(SocketAddr::new(ip, 443))
}

fn create_client(config: &Config, resolver: DynamicResolver) -> Result<Client> {
    let mut builder = Client::builder()
        .dns_resolver(Arc::new(resolver))
        .tls_backend_rustls()
        .pool_idle_timeout(Duration::from_secs(config.max_idle_time))
        .pool_max_idle_per_host(32)
        .connect_timeout(Duration::from_secs(config.conn_loss_time));

    if config.http11 { builder = builder.http1_only(); }
    else if config.http3 { builder = builder.http3_prior_knowledge(); }

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
) -> Result<()> {
    match forward_to_doh(client, resolver_url, data, stats.clone(), cache).await {
        Ok(bytes) => {
            socket.send_to(&bytes, peer).await?;
            Ok(())
        }
        Err(e) => {
            stats.errors.fetch_add(1, Ordering::Relaxed);
            debug!("UDP error from {}: {:#}", peer, e);
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
) -> Result<()> {
    let mut len_buf = [0u8; 2];
    stream.read_exact(&mut len_buf).await?;
    let len = u16::from_be_bytes(len_buf) as usize;
    
    let mut data = vec![0u8; len];
    stream.read_exact(&mut data).await?;
    let data = Bytes::from(data);

    match forward_to_doh(client, resolver_url, data, stats.clone(), cache).await {
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

async fn forward_to_doh(client: Client, resolver_url: Arc<String>, data: Bytes, _stats: Arc<Stats>, cache: DnsCache) -> Result<Bytes> {
    // 1. Check Cache
    if data.len() > 2 {
        let cache_key = data.slice(2..);
        if let Some((cached_resp, ttl)) = cache.get(&cache_key).await {
            let mut resp = vec![0u8; cached_resp.len()];
            resp.copy_from_slice(&cached_resp);
            resp[0] = data[0];
            resp[1] = data[1];
            
            let domain = extract_domain(&data);
            add_query_log(domain, format!("OK (Cache, TTL {})", ttl));
            return Ok(Bytes::from(resp));
        }
    }

    let start = std::time::Instant::now();
    let domain = extract_domain(&data);

    // Implement retries for robustness
    let mut last_err = None;
    for attempt in 0..2 {
        let resp = client
            .post(&*resolver_url)
            .header("content-type", "application/dns-message")
            .header("accept", "application/dns-message")
            .body(data.clone())
            .send()
            .await;

        match resp {
            Ok(r) => {
                if !r.status().is_success() {
                    last_err = Some(anyhow::anyhow!("Resolver status {}", r.status()));
                    continue;
                }
                let bytes = r.bytes().await?;
                let latency = start.elapsed().as_millis() as usize;
                LAST_LATENCY.store(latency, Ordering::Relaxed);
                add_query_log(domain, format!("OK ({}ms, att {})", latency, attempt + 1));
                
                // 2. Update Cache with TTL extraction
                if data.len() > 2 && bytes.len() > 2 {
                    let cache_key = data.slice(2..);
                    let mut ttl = 60; // Default TTL
                    if let Ok(msg) = Message::from_vec(&bytes) {
                        ttl = msg.answers().iter().map(|a| a.ttl()).min().unwrap_or(60);
                        if ttl < 10 { ttl = 10; }
                        if ttl > 3600 { ttl = 3600; }
                    }
                    cache.insert(cache_key, (bytes.clone(), ttl)).await;
                }

                return Ok(bytes);
            }
            Err(e) => {
                last_err = Some(e.into());
                tokio::time::sleep(Duration::from_millis(50 * (attempt + 1))).await;
            }
        }
    }

    add_query_log(domain, "Error (Net/HTTP)".to_string());
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Unknown error")))
}

