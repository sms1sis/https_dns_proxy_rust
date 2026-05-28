use std::net::{SocketAddr, IpAddr};
use anyhow::{Result, Context};
use tokio::net::{UdpSocket, TcpListener};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use reqwest::{Client, Url, Proxy};
use reqwest::dns::{Resolve, Resolving, Name, Addrs};
use std::sync::{Arc, Mutex};
use tokio::sync::{RwLock, Semaphore, mpsc, broadcast};

use std::time::{Duration, Instant};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::collections::{VecDeque, HashMap};
use std::sync::LazyLock;
use bytes::Bytes;
#[cfg(feature = "jni")]
use jni::errors::LogErrorAndDefault;
#[cfg(feature = "jni")]
use jni::{jni_sig, jni_str};
// No moka — plain std HashMap with manual TTL via Instant.
// Completely avoids moka's background-thread eviction issues on Android.
#[cfg(feature = "jni")]
use jni::JavaVM;
use hickory_resolver::proto::op::Message;
use hickory_resolver::config::{ResolverConfig, NameServerConfig, ResolverOpts, LookupIpStrategy, CLOUDFLARE, GOOGLE};
use hickory_resolver::config::ConnectionConfig;
use hickory_resolver::TokioResolver;
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use std::fs::File;
use std::io::Read;

pub struct Stats {
    pub queries_udp: AtomicUsize,
    pub queries_tcp: AtomicUsize,
    pub queries_https: AtomicUsize,
    pub cache_hits: AtomicUsize,
    pub cache_misses: AtomicUsize,
    pub malformed: AtomicUsize,
    pub errors: AtomicUsize,
    pub total_latency: AtomicUsize,
    pub latency_count: AtomicUsize,
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

enum NativeLog {
    Log { level: String, msg: String },
    Shutdown,
}

static NATIVE_LOG_SENDER: LazyLock<mpsc::UnboundedSender<NativeLog>> = LazyLock::new(|| {
    let (tx, mut rx) = mpsc::unbounded_channel::<NativeLog>();

    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            while let Some(log) = rx.recv().await {
                let (level, msg) = match log {
                    NativeLog::Shutdown => break,
                    NativeLog::Log { level, msg } => (level, msg),
                };

                // Emit to Android logcat first
                match level.as_str() {
                    "ERROR" => log::error!(target: "SafeDNS-Native", "{}", msg),
                    "WARN"  => log::warn! (target: "SafeDNS-Native", "{}", msg),
                    "INFO"  => log::info! (target: "SafeDNS-Native", "{}", msg),
                    _       => log::debug!(target: "SafeDNS-Native", "{}", msg),
                }

                // Forward to Kotlin via JNI — flat closure, errors short-circuit cleanly.
                #[cfg(feature = "jni")]
                let _ = (|| -> Option<()> {
                    let jvm_lock   = JVM.read().ok()?;
                    let jvm        = jvm_lock.as_ref()?;
                    let class_lock = PROXY_SERVICE_CLASS.read().ok()?;
                    let class_ref  = class_lock.as_ref()?;
                    let _ = jvm.attach_current_thread(|env| {
                        let level_j    = env.new_string(&level)?;
                        let tag_j      = env.new_string("SafeDNS-Native")?;
                        let msg_j      = env.new_string(&msg)?;
                        env.call_static_method(
                            class_ref,
                            jni_str!("nativeLog"),
                            jni_sig!("(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V"),
                            &[(&level_j).into(), (&tag_j).into(), (&msg_j).into()],
                        )?;
                        Ok::<(), jni::errors::Error>(())
                    }).ok()?;
                    Some(())
                })();
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
    let _ = NATIVE_LOG_SENDER.send(NativeLog::Log {
        level: level.to_string(),
        msg: msg.to_string(),
    });
}

/// Flush and shut down the native log thread cleanly.
/// Call this from stopProxy() to avoid leaking the thread.
fn shutdown_native_log() {
    let _ = NATIVE_LOG_SENDER.send(NativeLog::Shutdown);
}



#[cfg(feature = "jni")]
static GLOBAL_STATS: LazyLock<std::sync::RwLock<Option<Arc<Stats>>>> = LazyLock::new(|| std::sync::RwLock::new(None));
#[cfg(feature = "jni")]
static GLOBAL_CACHE: LazyLock<std::sync::RwLock<Option<DnsCache>>> = LazyLock::new(|| std::sync::RwLock::new(None));


static LAST_LATENCY: AtomicUsize = AtomicUsize::new(0);

#[cfg(feature = "jni")]
static JVM: LazyLock<std::sync::RwLock<Option<JavaVM>>> = LazyLock::new(|| std::sync::RwLock::new(None));
#[cfg(feature = "jni")]
static PROXY_SERVICE_CLASS: LazyLock<std::sync::RwLock<Option<jni::objects::Global<jni::objects::JClass>>>> = LazyLock::new(|| std::sync::RwLock::new(None));

fn add_query_log(domain: String, status: String) {
    let _ = LOG_SENDER.send(LogMessage { domain, status });
}

impl Stats {
    pub fn new() -> Self {
        Self {
            queries_udp: AtomicUsize::new(0),
            queries_tcp: AtomicUsize::new(0),
            queries_https: AtomicUsize::new(0),
            cache_hits: AtomicUsize::new(0),
            cache_misses: AtomicUsize::new(0),
            malformed: AtomicUsize::new(0),
            errors: AtomicUsize::new(0),
            total_latency: AtomicUsize::new(0),
            latency_count: AtomicUsize::new(0),
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
    /// Domains matching any of these suffixes are never cached.
    /// Suffix match: "every1dns.net" blocks "uuid.sub.every1dns.net" too.
    /// Pass an empty Vec to disable exclusions entirely.
    pub exclude_suffixes: Vec<String>,
}

/// A single cached DNS response with its expiry deadline.
#[derive(Clone)]
struct DnsCacheEntry {
    bytes:   Bytes,
    expires: Instant,
}

/// Sharded DNS cache: 16 independent buckets, each with its own Mutex.
///
/// Sharding reduces lock contention ~16x under concurrent query load —
/// tasks hashing to different shards never block each other.
/// TTL is enforced lazily on get(); expired entries are swept on insert().
#[derive(Clone)]
struct DnsCache {
    shards:          Arc<[Mutex<HashMap<String, DnsCacheEntry>>; 16]>,
    max_per_shard:   usize,
}

impl DnsCache {
    fn new(max_total: usize) -> Self {
        // Initialise 16 empty shards via array::from_fn
        let shards = std::array::from_fn(|_| Mutex::new(HashMap::new()));
        Self {
            shards: Arc::new(shards),
            max_per_shard: (max_total / 16).max(1),
        }
    }

    /// FNV-1a hash of the key, bottom 4 bits → shard index 0-15.
    #[inline]
    fn shard(key: &str) -> usize {
        key.bytes()
            .fold(0xcbf29ce484222325u64, |h, b| {
                (h ^ b as u64).wrapping_mul(0x100000001b3)
            }) as usize & 0xF
    }

    fn insert(&self, key: String, bytes: Bytes, ttl_secs: u64) {
        let expires = Instant::now() + Duration::from_secs(ttl_secs);
        let mut shard = self.shards[Self::shard(&key)].lock().unwrap();
        let now = Instant::now();
        shard.retain(|_, v| v.expires > now);
        if shard.len() >= self.max_per_shard {
            if let Some(oldest) = shard.iter()
                .min_by_key(|(_, v)| v.expires)
                .map(|(k, _)| k.clone())
            {
                shard.remove(&oldest);
            }
        }
        shard.insert(key, DnsCacheEntry { bytes, expires });
    }

    fn get(&self, key: &str) -> Option<Bytes> {
        let shard = self.shards[Self::shard(key)].lock().unwrap();
        shard.get(key).and_then(|e| {
            if e.expires > Instant::now() { Some(e.bytes.clone()) } else { None }
        })
    }



    fn entry_count(&self) -> usize {
        let now = Instant::now();
        self.shards.iter()
            .map(|s| s.lock().unwrap().values().filter(|v| v.expires > now).count())
            .sum()
    }

    fn invalidate_all(&self) {
        for shard in self.shards.iter() {
            shard.lock().unwrap().clear();
        }
    }
}

/// In-flight deduplication map.
///
/// When two concurrent queries arrive for the same cache key before either
/// has completed (the classic A + AAAA stampede), only the FIRST spawns a
/// real DoH fetch. Every subsequent waiter subscribes to a broadcast channel
/// and receives the response bytes the moment the first fetch completes —
/// zero extra DoH requests, zero extra latency for the waiters.
///
/// Layout:  key → broadcast::Sender<Option<Bytes>>
///   • `Some(bytes)` = fetch succeeded, here are the (ID-zeroed) response bytes
///   • `None`        = fetch failed; waiter should fall through and retry itself
#[derive(Clone)]
struct InFlight {
    inner: Arc<Mutex<HashMap<String, broadcast::Sender<Option<Bytes>>>>>,
}

impl InFlight {
    fn new() -> Self {
        Self { inner: Arc::new(Mutex::new(HashMap::new())) }
    }

    /// Try to become the "owner" of a fetch for `key`.
    ///
    /// Returns:
    ///   `Ok(tx)`  — caller is the owner; it must fetch and call `complete(key, result)`.
    ///   `Err(rx)` — another task is already fetching; caller should `rx.await` for result.
    fn register(&self, key: &str) -> Result<broadcast::Sender<Option<Bytes>>, broadcast::Receiver<Option<Bytes>>> {
        let mut map = self.inner.lock().unwrap();
        if let Some(tx) = map.get(key) {
            // Already in-flight — subscribe and wait
            Err(tx.subscribe())
        } else {
            // We are the owner — create channel, insert sender
            let (tx, _) = broadcast::channel(1);
            map.insert(key.to_string(), tx.clone());
            Ok(tx)
        }
    }

    /// Called by the owner once the fetch is done (success or failure).
    /// Broadcasts the result to all waiters and removes the key.
    fn complete(&self, key: &str, result: Option<Bytes>) {
        let mut map = self.inner.lock().unwrap();
        if let Some(tx) = map.remove(key) {
            let _ = tx.send(result);
        }
    }
}

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
    native_log("INFO", "run_proxy: Starting...");
    let addr: SocketAddr = format!("{}:{}", config.listen_addr, config.listen_port)
        .parse()
        .context("Failed to parse listen address")?;

    let resolver_url_parsed = Url::parse(&config.resolver_url)
        .context("Failed to parse resolver URL")?;
    let resolver_domain = resolver_url_parsed.domain().context("Resolver URL must have a domain")?.to_string();

    native_log("INFO", &format!("run_proxy: Binding sockets to {}...", addr));
    // Retry binding to handle transient port conflicts during restarts
    let mut udp_socket = None;
    let mut tcp_listener = None;
    for i in 0..5 {
        native_log("DEBUG", &format!("run_proxy: Bind attempt {}...", i + 1));
        let bind_result = (|| {
            native_log("DEBUG", "run_proxy: Creating UDP socket2::Socket...");
            let udp_sock = socket2::Socket::new(
                if addr.is_ipv4() { socket2::Domain::IPV4 } else { socket2::Domain::IPV6 },
                socket2::Type::DGRAM,
                Some(socket2::Protocol::UDP),
            )?;
            native_log("DEBUG", "run_proxy: Setting SO_REUSEADDR on UDP socket...");
            udp_sock.set_reuse_address(true)?;
            #[cfg(unix)]
            {
                native_log("DEBUG", "run_proxy: Setting SO_REUSEPORT on UDP socket...");
                udp_sock.set_reuse_port(true)?;
            }
            native_log("DEBUG", &format!("run_proxy: Binding UDP socket to {}...", addr));
            udp_sock.bind(&addr.into())?;
            native_log("DEBUG", "run_proxy: Converting to std::net::UdpSocket...");
            let udp_std: std::net::UdpSocket = udp_sock.into();
            native_log("DEBUG", "run_proxy: Setting non-blocking for UDP...");
            udp_std.set_nonblocking(true)?;
            native_log("DEBUG", "run_proxy: Creating tokio::net::UdpSocket::from_std...");
            let udp_tokio = UdpSocket::from_std(udp_std)?;

            native_log("DEBUG", "run_proxy: Creating TCP socket2::Socket...");
            let tcp_sock = socket2::Socket::new(
                if addr.is_ipv4() { socket2::Domain::IPV4 } else { socket2::Domain::IPV6 },
                socket2::Type::STREAM,
                Some(socket2::Protocol::TCP),
            )?;
            native_log("DEBUG", "run_proxy: Setting SO_REUSEADDR on TCP socket...");
            tcp_sock.set_reuse_address(true)?;
            #[cfg(unix)]
            {
                native_log("DEBUG", "run_proxy: Setting SO_REUSEPORT on TCP socket...");
                tcp_sock.set_reuse_port(true)?;
            }
            native_log("DEBUG", &format!("run_proxy: Binding TCP socket to {}...", addr));
            tcp_sock.bind(&addr.into())?;
            native_log("DEBUG", "run_proxy: Listening on TCP socket...");
            tcp_sock.listen(128)?;
            native_log("DEBUG", "run_proxy: Converting to std::net::TcpListener...");
            let tcp_std: std::net::TcpListener = tcp_sock.into();
            native_log("DEBUG", "run_proxy: Setting non-blocking for TCP...");
            tcp_std.set_nonblocking(true)?;
            native_log("DEBUG", "run_proxy: Creating tokio::net::TcpListener::from_std...");
            let tcp_tokio = TcpListener::from_std(tcp_std)?;

            Ok::<(UdpSocket, TcpListener), anyhow::Error>((udp_tokio, tcp_tokio))
        })();

        match bind_result {
            Ok((u, t)) => {
                udp_socket = Some(Arc::new(u));
                tcp_listener = Some(t);
                native_log("INFO", &format!("run_proxy: Sockets bound on attempt {}", i + 1));
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

    native_log("INFO", &format!("run_proxy: Resolving bootstrap for {}...", resolver_domain));
    let ips = resolve_bootstrap(&resolver_domain, &config.bootstrap_dns, config.allow_ipv6).await?;
    native_log("INFO", &format!("Bootstrapped {} to {:?}", resolver_domain, ips));
    
    let dynamic_resolver = DynamicResolver::new();
    dynamic_resolver.update(resolver_domain.clone(), ips).await;

    let client = create_client(&config, dynamic_resolver.clone())?;
    let resolver_url_str = Arc::new(config.resolver_url.clone());
    
    // DNS Cache: up to 2048 entries, sharded HashMap with manual TTL.
    // No moka background thread — works reliably on Android.
    let cache = DnsCache::new(2048);
    // In-flight dedup: concurrent identical queries share one DoH fetch
    let in_flight = InFlight::new();
    // Wrap exclude_suffixes in Arc so it can be cheaply cloned into every task
    let exclude_suffixes = Arc::new(config.exclude_suffixes.clone());

    #[cfg(feature = "jni")]
    {
        let mut w = GLOBAL_CACHE.write().unwrap();
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
                    Ok(new_ips) => {
                        native_log("DEBUG", &format!("Refreshed bootstrap IPs for {}: {:?}", domain, new_ips));
                        dynamic_resolver.update(domain.clone(), new_ips).await;
                    }
                    Err(e) => native_log("ERROR", &format!("Failed to refresh bootstrap IP: {}", e)),
                }
            }
        })
    };

    let tcp_semaphore = Arc::new(Semaphore::new(config.tcp_client_limit));

    let mut udp_loop = {
        let socket = udp_socket.clone();
        let client = client.clone();
        let resolver_url = resolver_url_str.clone();
        let stats = stats.clone();
        let cache = cache.clone();
        let in_flight = in_flight.clone();
        let cache_ttl = config.cache_ttl;
        let exclude_suffixes = exclude_suffixes.clone();
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
                        let in_flight = in_flight.clone();
                        let exclude_suffixes = exclude_suffixes.clone();
                        tokio::spawn(async move {
                            stats.queries_udp.fetch_add(1, Ordering::Relaxed);
                            if let Err(e) = handle_udp_query(socket, client, resolver_url, data, peer, stats, cache, in_flight, cache_ttl, exclude_suffixes).await {
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
        let in_flight = in_flight.clone();
        let cache_ttl = config.cache_ttl;
        let exclude_suffixes = exclude_suffixes.clone();
        tokio::spawn(async move {
            loop {
                match tcp_listener.accept().await {
                    Ok((mut stream, peer)) => {
                        let client = client.clone();
                        let resolver_url = resolver_url.clone();
                        let permit = semaphore.clone().acquire_owned().await;
                        let stats = stats.clone();
                        let cache = cache.clone();
                        let in_flight = in_flight.clone();
                        let exclude_suffixes = exclude_suffixes.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            stats.queries_tcp.fetch_add(1, Ordering::Relaxed);
                            if let Err(e) = handle_tcp_query(&mut stream, client, resolver_url, stats, cache, in_flight, cache_ttl, exclude_suffixes).await {
                                native_log("DEBUG", &format!("TCP error from {}: {}", peer, e));
                            }
                        });
                    }
                    Err(e) => native_log("ERROR", &format!("TCP accept error: {}", e)),
                }
            }
        })
    };

    // Optional periodic stats printout (CLI / debug use; disabled when interval == 0)
    let stats_handle = if config.statistic_interval > 0 {
        let stats = stats.clone();
        let interval_secs = config.statistic_interval;
        Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
            loop {
                interval.tick().await;
                let udp   = stats.queries_udp.load(Ordering::Relaxed);
                let tcp   = stats.queries_tcp.load(Ordering::Relaxed);
                let https = stats.queries_https.load(Ordering::Relaxed);
                let hits  = stats.cache_hits.load(Ordering::Relaxed);
                let mal   = stats.malformed.load(Ordering::Relaxed);
                let errs  = stats.errors.load(Ordering::Relaxed);
                let count = stats.latency_count.load(Ordering::Relaxed);
                let avg_lat = if count > 0 {
                    stats.total_latency.load(Ordering::Relaxed) / count
                } else { 0 };
                native_log("INFO", &format!(
                    "Stats: udp={} tcp={} https={} cache_hits={} malformed={} errors={} avg_latency={}ms",
                    udp, tcp, https, hits, mal, errs, avg_lat
                ));
            }
        }))
    } else {
        None
    };

    tokio::select! {
        _ = &mut shutdown_rx => native_log("INFO", &format!("Shutting down proxy...")),
        _ = &mut udp_loop => native_log("ERROR", &format!("UDP loop exited unexpectedly")),
        _ = &mut tcp_loop => native_log("ERROR", &format!("TCP loop exited unexpectedly")),
    }

    udp_loop.abort();
    tcp_loop.abort();
    bootstrap_handle.abort();
    if let Some(h) = stats_handle { h.abort(); }
    Ok(())
}

#[cfg(feature = "jni")]
pub mod jni_api {
    use super::*;
    use jni::objects::{JClass, JObject, JString};
    use jni::sys::{jboolean, jint};
    use tokio::runtime::Runtime;
    use tokio_util::sync::CancellationToken;

    static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| Runtime::new().unwrap());
    static CANCELLATION_TOKEN: LazyLock<Mutex<Option<CancellationToken>>> = LazyLock::new(|| Mutex::new(None));

    // Tracks whether run_proxy is actively running, so Kotlin can poll before
    // restarting after a stopProxy() call (avoids port-already-in-use races).
    static IS_PROXY_RUNNING: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_io_github_SafeDNS_ProxyService_initLogger<'local>(
        mut env: jni::EnvUnowned<'local>,
        _class: JClass<'local>,
        context: JObject<'local>,
    ) {
        env.with_env(|env| -> Result<_, jni::errors::Error> {
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

        if let Ok(class) = env.find_class(jni_str!("io/github/SafeDNS/ProxyService")) {
            if let Ok(global_ref) = env.new_global_ref(class) {
                if let Ok(mut w) = PROXY_SERVICE_CLASS.write() {
                    *w = Some(global_ref);
                }
            }
        }

        #[cfg(target_os = "android")]
        rustls_platform_verifier::android::init_with_env(env, context)?;

        native_log("INFO", "Logger, JVM and Global Class Ref initialized");
        Ok(())
        }).resolve::<jni::errors::LogErrorAndDefault>();

        #[cfg(not(target_os = "android"))]
        let _ = context;
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_io_github_SafeDNS_ProxyService_startProxy<'local>(
        mut unowned_env: jni::EnvUnowned<'local>,
        _class: JClass<'local>,
        listen_addr: JString<'local>,
        listen_port: jint,
        resolver_url: JString<'local>,
        bootstrap_dns: JString<'local>,
        allow_ipv6: jni::sys::jboolean,
        cache_ttl: jni::sys::jlong,
        tcp_limit: jint,
        poll_interval: jni::sys::jlong,
        use_http3: jni::sys::jboolean,
        exclude_suffixes: JString<'local>,
    ) -> jint {
        unowned_env.with_env(|env| -> Result<jint, jni::errors::Error> {
        let listen_addr: String = listen_addr.try_to_string(env)?;
        let resolver_url: String = resolver_url.try_to_string(env)?;
        let bootstrap_dns: String = bootstrap_dns.try_to_string(env)?;
        let exclude_raw: String = exclude_suffixes.try_to_string(env)?;

        let exclude_suffixes: Vec<String> = exclude_raw
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();

        native_log("INFO", &format!("startProxy: addr={}, port={}, resolver={}, exclude={:?}", listen_addr, listen_port, resolver_url, exclude_suffixes));

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
            exclude_suffixes,
        };

        let token = CancellationToken::new();
        let cloned_token = token.clone();
        {
            let mut lock = CANCELLATION_TOKEN.lock().unwrap();
            *lock = Some(token);
        }

        let stats = Arc::new(Stats::new());
        {
            let mut w = GLOBAL_STATS.write().unwrap();
            *w = Some(stats.clone());
        }

        let config_clone = config.clone();
        let stats_clone = stats.clone();

        RUNTIME.spawn(async move {
            let (tx, rx) = tokio::sync::oneshot::channel();

            tokio::spawn(async move {
                cloned_token.cancelled().await;
                let _ = tx.send(());
            });

            IS_PROXY_RUNNING.store(true, Ordering::SeqCst);
            if let Err(e) = run_proxy(config_clone, stats_clone, rx).await {
                native_log("ERROR", &format!("Proxy error: {}", e));
            }
            IS_PROXY_RUNNING.store(false, Ordering::SeqCst);
        });

        Ok(0)
        }).resolve::<jni::errors::LogErrorAndDefault>()
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_io_github_SafeDNS_ProxyService_getStats<'local>(
        mut unowned_env: jni::EnvUnowned<'local>,
        _class: JClass<'local>,
    ) -> jni::sys::jintArray {
        unowned_env.with_env(|env| -> Result<jni::sys::jintArray, jni::errors::Error> {
        let stats_opt = GLOBAL_STATS.read().unwrap().clone();

        let cache_count = GLOBAL_CACHE.read().unwrap()
            .as_ref()
            .map(|c| c.entry_count())
            .unwrap_or(0);

        let mut values = [0i32; 10];
        if let Some(stats) = stats_opt {
            let udp    = stats.queries_udp.load(Ordering::Relaxed);
            let tcp    = stats.queries_tcp.load(Ordering::Relaxed);
            let hits   = stats.cache_hits.load(Ordering::Relaxed);
            let misses = stats.cache_misses.load(Ordering::Relaxed);
            let https  = stats.queries_https.load(Ordering::Relaxed);
            let errs   = stats.errors.load(Ordering::Relaxed);
            let t_lat  = stats.total_latency.load(Ordering::Relaxed);
            let count  = stats.latency_count.load(Ordering::Relaxed);
            let avg_lat = if count > 0 { (t_lat / count) as i32 } else { 0 };

            values[0] = udp as i32;
            values[1] = tcp as i32;
            values[2] = stats.malformed.load(Ordering::Relaxed) as i32;
            values[3] = (udp + tcp) as i32;
            values[4] = https as i32;
            values[5] = hits as i32;
            values[6] = errs as i32;
            values[7] = avg_lat;
            values[8] = cache_count as i32;
            values[9] = misses as i32;
        }

        let array = env.new_int_array(10)?;
        array.set_region(env, 0, &values)?;
        Ok(array.into_raw())
        }).resolve::<jni::errors::LogErrorAndDefault>()
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_io_github_SafeDNS_ProxyService_clearStats<'local>(
        mut _env: jni::EnvUnowned<'local>,
        _class: JClass<'local>,
    ) {
        if let Some(stats) = GLOBAL_STATS.read().unwrap().clone() {
            stats.queries_udp.store(0, Ordering::Relaxed);
            stats.queries_tcp.store(0, Ordering::Relaxed);
            stats.queries_https.store(0, Ordering::Relaxed);
            stats.cache_hits.store(0, Ordering::Relaxed);
            stats.cache_misses.store(0, Ordering::Relaxed);
            stats.malformed.store(0, Ordering::Relaxed);
            stats.errors.store(0, Ordering::Relaxed);
            stats.total_latency.store(0, Ordering::Relaxed);
            stats.latency_count.store(0, Ordering::Relaxed);
            native_log("INFO", "Traffic statistics cleared");
        }
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_io_github_SafeDNS_ProxyService_getLatency<'local>(
        mut _env: jni::EnvUnowned<'local>,
        _class: JClass<'local>,
    ) -> jint {
        LAST_LATENCY.load(Ordering::Relaxed) as jint
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_io_github_SafeDNS_ProxyService_getLogs<'local>(
        mut env: jni::EnvUnowned<'local>,
        _class: JClass<'local>,
    ) -> jni::sys::jobjectArray {
        let logs: Vec<String> = {
            let queue = QUERY_LOGS.lock().unwrap();
            queue.iter().cloned().collect()
        };

        if logs.is_empty() {
            return std::ptr::null_mut();
        }

        env.with_env(|env| {
            let cls = env.find_class(jni_str!("java/lang/String"))?;
            let array = env.new_object_array(
                logs.len() as jni::sys::jsize,
                &cls,
                jni::objects::JObject::null()
            )?;

            for (i, log) in logs.iter().enumerate() {
                let s = env.new_string(log)?;
                array.set_element(env, i, &s)?;
            }

            Ok::<_, jni::errors::Error>(array.into_raw())
        }).resolve::<LogErrorAndDefault>()
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_io_github_SafeDNS_ProxyService_clearLogs<'local>(
        mut _env: jni::EnvUnowned<'local>,
        _class: JClass<'local>,
    ) {
        let mut logs = QUERY_LOGS.lock().unwrap();
        logs.clear();
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_io_github_SafeDNS_ProxyService_stopProxy<'local>(
        mut _env: jni::EnvUnowned<'local>,
        _class: JClass<'local>,
    ) {
        let mut lock = CANCELLATION_TOKEN.lock().unwrap();
        if let Some(token) = lock.take() {
            token.cancel();
        }
        shutdown_native_log();
    }

    /// Returns true if run_proxy is still executing (i.e. not yet fully shut down).
    /// Kotlin should poll this after stopProxy() before calling startProxy() again
    /// to avoid binding the same port while the old instance is still releasing it.
    ///
    /// Kotlin usage:
    ///   stopProxy()
    ///   var waited = 0
    ///   while (isProxyRunning() && waited < 3000) { delay(100); waited += 100 }
    ///   startProxy(...)
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_io_github_SafeDNS_ProxyService_isProxyRunning<'local>(
        mut _env: jni::EnvUnowned<'local>,
        _class: JClass<'local>,
    ) -> jboolean {
        IS_PROXY_RUNNING.load(Ordering::SeqCst) as jboolean
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_io_github_SafeDNS_ProxyService_clearCache<'local>(
        mut _env: jni::EnvUnowned<'local>,
        _class: JClass<'local>,
    ) {
        if let Some(cache) = GLOBAL_CACHE.read().unwrap().as_ref() {
            cache.invalidate_all();
            native_log("INFO", "DNS Cache cleared via JNI");
        }
    }
}

async fn resolve_bootstrap(domain: &str, bootstrap_dns: &str, allow_ipv6: bool) -> Result<Vec<SocketAddr>> {
    native_log("INFO", &format!("resolve_bootstrap: domain={}, bootstrap_dns={}, ipv6={}", domain, bootstrap_dns, allow_ipv6));
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

    native_log("INFO", &format!("resolve_bootstrap: Using bootstrap servers: {:?}", servers));

    let mut config = ResolverConfig::from_parts(None, vec![], vec![]);
    for s in servers {
        config.add_name_server(NameServerConfig::new(
            s.ip(), 
            true, 
            vec![ConnectionConfig::udp(), ConnectionConfig::tcp()]
        ));
    }

    let mut opts = ResolverOpts::default();
    opts.ip_strategy = if allow_ipv6 {
        LookupIpStrategy::Ipv4thenIpv6
    } else {
        LookupIpStrategy::Ipv4Only
    };

    let resolver = TokioResolver::builder_with_config(config, TokioRuntimeProvider::default())
        .with_options(opts)
        .build();
    
    native_log("INFO", &format!("resolve_bootstrap: Querying Hickory for {}...", domain));
    let ips = match resolver.as_ref().map_err(|e| anyhow::anyhow!("Failed to build resolver: {}", e))?.lookup_ip(domain).await {
        Ok(ips) => {
            native_log("INFO", &format!("resolve_bootstrap: Hickory resolved {} to {:?}", domain, ips));
            ips
        },
        Err(e) => {
            native_log("WARN", &format!("Full dual-stack lookup failed for {}, retrying with fallback nameservers: {:?}", domain, e));
            let mut opts4 = ResolverOpts::default();
            opts4.ip_strategy = LookupIpStrategy::Ipv4Only;
            
            // Try Cloudflare AND Google as fallbacks
            let mut fallback_config = ResolverConfig::udp_and_tcp(&CLOUDFLARE);
            for ns in GOOGLE.udp_and_tcp() {
                fallback_config.add_name_server(ns);
            }

            let resolver4 = TokioResolver::builder_with_config(fallback_config, TokioRuntimeProvider::default())
                .with_options(opts4)
                .build()
                .map_err(|e| anyhow::anyhow!("Failed to build fallback resolver: {}", e))?;
            
            native_log("INFO", "resolve_bootstrap: Querying Hickory (IPv4 fallback) for domain...");
            resolver4.lookup_ip(domain).await.context("Failed to resolve DoH provider (IPv4 retry)")?
        }
    };
    
    let addrs: Vec<SocketAddr> = ips.iter().map(|ip| SocketAddr::new(ip, 443)).collect();
    
    if addrs.is_empty() {
        return Err(anyhow::anyhow!("No IPs found for {}", domain));
    }
    
    Ok(addrs)
}

fn create_client(config: &Config, resolver: DynamicResolver) -> Result<Client> {
    let mut builder = Client::builder()
        .user_agent(format!("SafeDNS/{}", env!("CARGO_PKG_VERSION")))
        .dns_resolver(Arc::new(resolver))
        .tcp_nodelay(true)
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(32)
        .tcp_keepalive(Some(Duration::from_secs(60)))
        .connect_timeout(Duration::from_secs(5));

    if config.http11 { 
        builder = builder.http1_only(); 
    } else if config.http3 {
        // HTTP/3 requires ALPN negotiation or prior knowledge
        // In reqwest 0.12, enabling http3 feature and setting version on request is common
        // but we can also set it here.
        builder = builder.http2_adaptive_window(true);
    } else {
        // Force HTTP/2 only if not http3 or http11
        builder = builder.http2_prior_knowledge().http2_adaptive_window(true);
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
    in_flight: InFlight,
    cache_ttl_default: u64,
    exclude_suffixes: Arc<Vec<String>>,
) -> Result<()> {
    match forward_to_doh(client, resolver_url, data, stats.clone(), cache, in_flight, cache_ttl_default, exclude_suffixes).await {
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
    in_flight: InFlight,
    cache_ttl_default: u64,
    exclude_suffixes: Arc<Vec<String>>,
) -> Result<()> {
    let mut len_buf = [0u8; 2];
    stream.read_exact(&mut len_buf).await?;
    let len = u16::from_be_bytes(len_buf) as usize;
    
    let mut data = vec![0u8; len];
    stream.read_exact(&mut data).await?;
    let data = Bytes::from(data);

    if extract_dns_info(&data).0 == "unknown" {
        stats.malformed.fetch_add(1, Ordering::Relaxed);
    }

    match forward_to_doh(client, resolver_url, data, stats.clone(), cache, in_flight, cache_ttl_default, exclude_suffixes).await {
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

/// Parse a DNS query wire message and return `(display_domain, cache_key)`.
///
/// The cache key is always `"<lower-name>:<qtype>:<qclass>"` — derived from
/// the question section only, never from the transaction ID or flag bytes,
/// so repeated queries for the same name hit the same cache slot regardless
/// of which client or flag combination sent them.
///
/// Falls back to a best-effort key built from the raw question bytes (hex)
/// when the message cannot be parsed by hickory — still stable across calls
/// for the same wire content.
fn extract_dns_info(data: &[u8]) -> (String, String) {
    if let Ok(msg) = Message::from_vec(data) {
        if let Some(query) = msg.queries.first() {
            let name = query.name().to_string();
            // Strip trailing dot for the human-readable domain display.
            let domain = if name.ends_with('.') && name.len() > 1 {
                name[..name.len() - 1].to_string()
            } else {
                name.clone()
            };
            // Key always uses the dot-stripped, lowercased domain so that
            // "google.com." and "google.com" (if they ever differ) hash to
            // the same slot. Never includes the transaction ID or flags.
            let key = format!("{}:{}:{}", domain.to_lowercase(), query.query_type(), query.query_class());
            native_log("DEBUG", &format!("extract_dns_info: domain={} key={}", domain, key));
            return (domain, key);
        }
    }

    // Fallback: parse domain name manually from wire format
    let mut domain = String::from("unknown");
    if data.len() > 12 {
        let mut d = String::new();
        let mut i = 12;
        while i < data.len() && data[i] != 0 {
            let len = data[i] as usize;
            i += 1;
            if i + len > data.len() { break; }
            if !d.is_empty() { d.push('.'); }
            d.push_str(&String::from_utf8_lossy(&data[i..i + len]));
            i += len;
        }
        if !d.is_empty() { domain = d; }
    }

    // Fallback key: hex of question section bytes (bytes 12+), stable across
    // repeated calls for the same packet content.
    let key = if data.len() > 12 {
        data[12..].iter().map(|b| format!("{:02x}", b)).collect::<String>()
    } else {
        data.iter().map(|b| format!("{:02x}", b)).collect::<String>()
    };

    native_log("DEBUG", &format!("extract_dns_info: FALLBACK domain={} key_len={}", domain, key.len()));
    (domain, key)
}

/// Rewrite every TTL field in a DNS response wire message to 0.
///
/// When the proxy returns a response to Android with TTL > 0, Android's own
/// DNS resolver caches the result and serves subsequent queries itself —
/// bypassing the proxy entirely, so the proxy cache never gets a chance to hit.
///
/// By zeroing all TTLs in the response we hand back to Android, we tell Android
/// "this answer expires immediately" — forcing it to ask the proxy again on the
/// next lookup. The proxy then serves the answer from its own cache (which uses
/// the real TTL from the DoH server), so latency stays near zero for cached
/// entries while Android never suppresses queries.
///
/// DNS wire format (RFC 1035 §4.1):
///   Header   (12 bytes)
///   Questions (variable — skip by parsing label sequences)
///   Answers / Authority / Additional records:
///     NAME    (label sequence or pointer)
///     TYPE    (2 bytes)
///     CLASS   (2 bytes)
///     TTL     (4 bytes)  ← we zero these
///     RDLENGTH (2 bytes)
///     RDATA   (RDLENGTH bytes)
fn zero_out_ttls(data: &mut Vec<u8>) {
    if data.len() < 12 { return; }

    let ancount = u16::from_be_bytes([data[6], data[7]]) as usize;
    let nscount = u16::from_be_bytes([data[8], data[9]]) as usize;
    let arcount = u16::from_be_bytes([data[10], data[11]]) as usize;
    let total_rr = ancount + nscount + arcount;

    if total_rr == 0 { return; }

    let qdcount = u16::from_be_bytes([data[4], data[5]]) as usize;
    let mut pos = 12;

    // Skip question section
    for _ in 0..qdcount {
        // Skip QNAME label sequence
        while pos < data.len() {
            let len = data[pos] as usize;
            if len == 0 { pos += 1; break; }
            // Compression pointer (top 2 bits = 11)?
            if (data[pos] & 0xC0) == 0xC0 { pos += 2; break; }
            pos += 1 + len;
        }
        pos += 4; // QTYPE + QCLASS
    }

    // Walk resource records and zero each TTL
    for _ in 0..total_rr {
        if pos >= data.len() { break; }
        // Skip NAME (label sequence or compression pointer)
        loop {
            if pos >= data.len() { return; }
            let len = data[pos] as usize;
            if len == 0 { pos += 1; break; }
            if (data[pos] & 0xC0) == 0xC0 { pos += 2; break; }
            pos += 1 + len;
        }
        // Need TYPE(2) + CLASS(2) + TTL(4) + RDLENGTH(2) = 10 bytes
        if pos + 10 > data.len() { break; }
        pos += 4; // skip TYPE + CLASS
        // Zero the 4-byte TTL
        data[pos]     = 0;
        data[pos + 1] = 0;
        data[pos + 2] = 0;
        data[pos + 3] = 0;
        pos += 4; // skip TTL
        let rdlength = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2 + rdlength; // skip RDLENGTH + RDATA
    }
}

async fn forward_to_doh(
    client: Client,
    resolver_url: Arc<String>,
    data: Bytes,
    stats: Arc<Stats>,
    cache: DnsCache,
    in_flight: InFlight,
    cache_ttl_default: u64,
    exclude_suffixes: Arc<Vec<String>>,
) -> Result<Bytes> {
    if data.len() < 12 {
        return Err(anyhow::anyhow!("DNS message too short"));
    }

    let original_id = [data[0], data[1]];
    let (domain, cache_key) = extract_dns_info(&data);
    if domain == "unknown" {
        stats.malformed.fetch_add(1, Ordering::Relaxed);
    }

    // Suffix-based exclusion: skip caching for any domain whose lowercase
    // representation ends with one of the configured suffixes.
    let domain_lc = domain.to_lowercase();
    let should_cache = exclude_suffixes.is_empty() || !exclude_suffixes.iter().any(|suffix| {
        domain_lc == *suffix || domain_lc.ends_with(&format!(".{}", suffix))
    });

    // ── 1. Cache lookup ──────────────────────────────────────────────────────
    if should_cache {
        if let Some(cached_bytes) = cache.get(&cache_key) {
            let mut resp = cached_bytes.to_vec();
            if resp.len() >= 2 {
                resp[0] = original_id[0];
                resp[1] = original_id[1];
            }
            // Zero TTLs so Android's system resolver doesn't cache this
            // and is forced to ask the proxy again next time (proxy serves
            // from its own cache instantly, so latency stays near zero).
            zero_out_ttls(&mut resp);
            stats.cache_hits.fetch_add(1, Ordering::Relaxed);
            native_log("DEBUG", &format!("cache HIT: key={}", cache_key));
            add_query_log(domain, "OK (Cache)".into());
            return Ok(Bytes::from(resp));
        } else {
            stats.cache_misses.fetch_add(1, Ordering::Relaxed);
        }
    }

    // ── 2. In-flight deduplication ───────────────────────────────────────────
    // If another task is already fetching the same key, wait for its result
    // instead of firing a duplicate DoH request.
    if should_cache {
        match in_flight.register(&cache_key) {
            Err(mut rx) => {
                // We are a waiter — block until the owner completes
                native_log("DEBUG", &format!("in-flight WAIT: key={}", cache_key));
                match rx.recv().await {
                    Ok(Some(response_bytes)) => {
                        // Owner succeeded — patch our transaction ID and return
                        let mut resp = response_bytes.to_vec();
                        if resp.len() >= 2 {
                            resp[0] = original_id[0];
                            resp[1] = original_id[1];
                        }
                        zero_out_ttls(&mut resp);
                        stats.cache_hits.fetch_add(1, Ordering::Relaxed);
                        native_log("DEBUG", &format!("in-flight HIT: key={}", cache_key));
                        add_query_log(domain, "OK (Cache)".into());
                        return Ok(Bytes::from(resp));
                    }
                    // Owner failed or channel lagged — fall through to fetch ourselves
                    Ok(None) | Err(_) => {
                        native_log("DEBUG", &format!("in-flight MISS (owner failed): key={}", cache_key));
                    }
                }
            }
            Ok(tx) => {
                // We are the owner — fetch below, then broadcast result
                // `tx` is kept alive until we call in_flight.complete()
                let _ = tx; // suppress unused warning; ownership held until complete()
            }
        }
    }

    // ── 3. Forward to DoH resolver ───────────────────────────────────────────
    stats.queries_https.fetch_add(1, Ordering::Relaxed);

    // RFC 8484 §4.1 — transaction ID MUST be 0 in DoH requests.
    let mut request_data = data.to_vec();
    request_data[0] = 0;
    request_data[1] = 0;
    let request_bytes = Bytes::from(request_data);

    let start = Instant::now();
    let mut last_err: Option<anyhow::Error> = None;

    for attempt in 0..3u64 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(100 * attempt)).await;
        }
        let resp = client
            .post(&*resolver_url)
            .header("content-type", "application/dns-message")
            .header("accept", "application/dns-message")
            .body(request_bytes.clone())
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
                stats.total_latency.fetch_add(latency, Ordering::Relaxed);
                stats.latency_count.fetch_add(1, Ordering::Relaxed);
                add_query_log(domain.clone(), format!("OK ({}ms, v{:?}, att {})", latency, version, attempt + 1));

                // ── 4. Insert into cache & broadcast to waiters ───────────────
                if should_cache && bytes.len() > 2 {
                    let ttl = if let Ok(msg) = Message::from_vec(&bytes) {
                        // Positive response: use minimum answer TTL
                        let ans_ttl = msg.answers.iter()
                            .map(|a| a.ttl as u64)
                            .min();
                        // Negative response (NXDOMAIN / NODATA): use SOA minimum TTL
                        // from the authority section so we cache "this doesn't exist"
                        // correctly instead of always fetching it from DoH.
                        let soa_ttl = msg.authorities.iter()
                            .map(|a| a.ttl as u64)
                            .min();
                        ans_ttl.or(soa_ttl).unwrap_or(cache_ttl_default).clamp(1, 3600)
                    } else {
                        cache_ttl_default.clamp(1, 3600)
                    };
                    // Store with ID=0 (as received from DoH) — callers patch their own ID
                    cache.insert(cache_key.clone(), bytes.clone(), ttl);
                    native_log("DEBUG", &format!("cache INSERT: key={} ttl={}s", cache_key, ttl));
                    // Broadcast to any waiters (send the ID=0 bytes; each waiter patches its own ID)
                    in_flight.complete(&cache_key, Some(bytes.clone()));
                } else if should_cache {
                    // Response too short to cache — unblock waiters with failure
                    in_flight.complete(&cache_key, None);
                }

                // Restore original transaction ID before returning to caller,
                // then zero out TTLs so Android doesn't cache the response itself.
                let mut final_resp = bytes.to_vec();
                if final_resp.len() >= 2 {
                    final_resp[0] = original_id[0];
                    final_resp[1] = original_id[1];
                }
                zero_out_ttls(&mut final_resp);
                return Ok(Bytes::from(final_resp));
            }
            Err(e) => {
                last_err = Some(e.into());
            }
        }
    }

    // All attempts failed — unblock any waiters
    if should_cache {
        in_flight.complete(&cache_key, None);
    }

    let err_msg = match last_err.as_ref() {
        Some(e) => {
            let s = e.to_string();
            if s.contains("connection closed") || s.contains("broken pipe") {
                format!("Conn Closed: {}", s)
            } else if s.contains("timed out") {
                format!("Timeout: {}", s)
            } else {
                s
            }
        }
        None => "Unknown Error".into(),
    };

    add_query_log(domain, format!("Error: {}", err_msg));
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Unknown error")))
}

