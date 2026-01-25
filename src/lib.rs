use std::net::{SocketAddr, IpAddr};
use anyhow::{Result, Context};
use tokio::net::{UdpSocket, TcpListener};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use reqwest::{Client, Url, Proxy};
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use tracing::{info, error, debug};
use trust_dns_resolver::config::{ResolverConfig, NameServerConfig, ResolverOpts, Protocol};
use trust_dns_resolver::TokioAsyncResolver;
use std::time::Duration;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::fs::File;
use std::io::Read;

pub struct Stats {
    pub queries_udp: AtomicUsize,
    pub queries_tcp: AtomicUsize,
    pub errors: AtomicUsize,
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
    pub resolver_url: String,
    pub proxy_server: Option<String>,
    pub source_addr: Option<String>,
    pub http11: bool,
    pub http3: bool,
    pub max_idle_time: u64,
    pub conn_loss_time: u64,
    pub ca_path: Option<String>,
    pub statistic_interval: u64,
}

pub async fn run_proxy(config: Config, stats: Arc<Stats>, mut shutdown_rx: tokio::sync::oneshot::Receiver<()>) -> Result<()> {
    let addr: SocketAddr = format!("{}:{}", config.listen_addr, config.listen_port)
        .parse()
        .context("Failed to parse listen address")?;

    let resolver_url_parsed = Url::parse(&config.resolver_url)
        .context("Failed to parse resolver URL")?;
    let resolver_domain = resolver_url_parsed.domain().context("Resolver URL must have a domain")?.to_string();

    let udp_socket = Arc::new(UdpSocket::bind(addr).await.context("Failed to bind UDP socket")?);
    let tcp_listener = TcpListener::bind(addr).await.context("Failed to bind TCP listener")?;

    info!("Listening on UDP/TCP {} -> {}", addr, config.resolver_url);

    let ip = resolve_bootstrap(&resolver_domain, &config.bootstrap_dns, config.force_ipv4).await?;
    info!("Bootstrapped {} to {}", resolver_domain, ip);
    let client = create_client(&config, Some((resolver_domain.clone(), ip)))?;

    let shared_client = Arc::new(RwLock::new(client));
    let resolver_url_str = Arc::new(config.resolver_url.clone());

    // Bootstrap Refresh Loop
    let bootstrap_handle = {
        let shared_client = shared_client.clone();
        let config = config.clone();
        let domain = resolver_domain.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(config.polling_interval));
            loop {
                interval.tick().await;
                match resolve_bootstrap(&domain, &config.bootstrap_dns, config.force_ipv4).await {
                    Ok(new_ip) => {
                        debug!("Refreshed bootstrap IP for {}: {}", domain, new_ip);
                        match create_client(&config, Some((domain.clone(), new_ip))) {
                            Ok(new_client) => {
                                let mut w = shared_client.write().await;
                                *w = new_client;
                            }
                            Err(e) => error!("Failed to create client: {}", e),
                        }
                    }
                    Err(e) => error!("Failed to refresh bootstrap IP: {}", e),
                }
            }
        })
    };

    // Stats Loop
    let stats_handle = if config.statistic_interval > 0 {
        let stats = stats.clone();
        Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(config.statistic_interval));
            loop {
                interval.tick().await;
                info!("Stats: UDP: {}, TCP: {}, Errors: {}", 
                    stats.queries_udp.load(Ordering::Relaxed),
                    stats.queries_tcp.load(Ordering::Relaxed),
                    stats.errors.load(Ordering::Relaxed)
                );
            }
        }))
    } else {
        None
    };

    let tcp_semaphore = Arc::new(Semaphore::new(config.tcp_client_limit));

    let udp_loop = {
        let socket = udp_socket.clone();
        let shared_client = shared_client.clone();
        let resolver_url = resolver_url_str.clone();
        let stats = stats.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                match socket.recv_from(&mut buf).await {
                    Ok((len, peer)) => {
                        let data = buf[..len].to_vec();
                        let socket = socket.clone();
                        let shared_client = shared_client.clone();
                        let resolver_url = resolver_url.clone();
                        let stats = stats.clone();
                        tokio::spawn(async move {
                            stats.queries_udp.fetch_add(1, Ordering::Relaxed);
                            let client = shared_client.read().await.clone();
                            if let Err(e) = handle_udp_query(socket, client, resolver_url, data, peer, stats).await {
                                debug!("UDP error from {}: {}", peer, e);
                            }
                        });
                    }
                    Err(e) => error!("UDP recv error: {}", e),
                }
            }
        })
    };

    let tcp_loop = {
        let shared_client = shared_client.clone();
        let resolver_url = resolver_url_str.clone();
        let semaphore = tcp_semaphore.clone();
        let stats = stats.clone();
        tokio::spawn(async move {
            loop {
                match tcp_listener.accept().await {
                    Ok((mut stream, peer)) => {
                        let shared_client = shared_client.clone();
                        let resolver_url = resolver_url.clone();
                        let permit = semaphore.clone().acquire_owned().await;
                        let stats = stats.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            stats.queries_tcp.fetch_add(1, Ordering::Relaxed);
                            let client = shared_client.read().await.clone();
                            if let Err(e) = handle_tcp_query(&mut stream, client, resolver_url, stats).await {
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
    if let Some(h) = stats_handle { h.abort(); }

    Ok(())
}

#[cfg(feature = "jni")]
pub mod jni_api {
    use super::*;
    use jni::JNIEnv;
    use jni::objects::{JClass, JString};
    use jni::sys::jint;
    use tokio::runtime::Runtime;
    use tokio_util::sync::CancellationToken;
    use std::sync::Mutex;

    lazy_static::lazy_static! {
        static ref RUNTIME: Runtime = Runtime::new().unwrap();
        static ref CANCELLATION_TOKEN: Mutex<Option<CancellationToken>> = Mutex::new(None);
    }

    #[no_mangle]
    pub extern "system" fn Java_com_example_httpsdnsproxy_ProxyService_initLogger(
        _env: JNIEnv,
        _class: JClass,
    ) {
        #[cfg(target_os = "android")]
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(tracing::log::LevelFilter::Debug)
                .with_tag("https_dns_proxy"),
        );
    }

    #[no_mangle]
    pub extern "system" fn Java_com_example_httpsdnsproxy_ProxyService_startProxy(
        mut env: JNIEnv,
        _class: JClass,
        listen_addr: JString,
        listen_port: jint,
        resolver_url: JString,
        bootstrap_dns: JString,
    ) -> jint {
        let listen_addr: String = env.get_string(&listen_addr).unwrap().into();
        let resolver_url: String = env.get_string(&resolver_url).unwrap().into();
        let bootstrap_dns: String = env.get_string(&bootstrap_dns).unwrap().into();

        let config = Config {
            listen_addr,
            listen_port: listen_port as u16,
            tcp_client_limit: 20,
            bootstrap_dns,
            polling_interval: 120,
            force_ipv4: false,
            resolver_url,
            proxy_server: None,
            source_addr: None,
            http11: false,
            http3: false,
            max_idle_time: 118,
            conn_loss_time: 15,
            ca_path: None,
            statistic_interval: 0,
        };

        let token = CancellationToken::new();
        let cloned_token = token.clone();
        {
            let mut lock = CANCELLATION_TOKEN.lock().unwrap();
            *lock = Some(token);
        }

        RUNTIME.spawn(async move {
            let stats = Arc::new(Stats::new());
            let (tx, rx) = tokio::sync::oneshot::channel();
            
            tokio::spawn(async move {
                cloned_token.cancelled().await;
                let _ = tx.send(());
            });

            if let Err(e) = run_proxy(config, stats, rx).await {
                error!("Proxy error: {}", e);
            }
        });

        0
    }

    #[no_mangle]
    pub extern "system" fn Java_com_example_httpsdnsproxy_ProxyService_stopProxy(
        _env: JNIEnv,
        _class: JClass,
    ) {
        let mut lock = CANCELLATION_TOKEN.lock().unwrap();
        if let Some(token) = lock.take() {
            token.cancel();
        }
    }
}

async fn resolve_bootstrap(domain: &str, bootstrap_dns: &str, force_ipv4: bool) -> Result<SocketAddr> {
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
    if force_ipv4 {
        opts.ip_strategy = trust_dns_resolver::config::LookupIpStrategy::Ipv4Only;
    }

    let resolver = TokioAsyncResolver::tokio(config, opts);
    let ips = resolver.lookup_ip(domain).await.context("Failed to resolve DoH provider")?;
    
    let ip = if force_ipv4 {
        ips.iter().find(|ip| ip.is_ipv4()).ok_or_else(|| anyhow::anyhow!("No IPv4 found"))?
    } else {
        ips.iter().next().ok_or_else(|| anyhow::anyhow!("No IPs found"))?
    };
    
    Ok(SocketAddr::new(ip, 443))
}

fn create_client(config: &Config, resolve_override: Option<(String, SocketAddr)>) -> Result<Client> {
    let mut builder = Client::builder()
        .use_rustls_tls()
        .pool_idle_timeout(Duration::from_secs(config.max_idle_time))
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

    if let Some((domain, addr)) = resolve_override {
        builder = builder.resolve(&domain, addr);
    }

    Ok(builder.build()?)
}

async fn handle_udp_query(
    socket: Arc<UdpSocket>,
    client: Client,
    resolver_url: Arc<String>,
    data: Vec<u8>,
    peer: SocketAddr,
    stats: Arc<Stats>,
) -> Result<()> {
    match forward_to_doh(client, resolver_url, data).await {
        Ok(bytes) => {
            socket.send_to(&bytes, peer).await?;
            Ok(())
        }
        Err(e) => {
            stats.errors.fetch_add(1, Ordering::Relaxed);
            Err(e)
        }
    }
}

async fn handle_tcp_query(
    stream: &mut tokio::net::TcpStream,
    client: Client,
    resolver_url: Arc<String>,
    stats: Arc<Stats>,
) -> Result<()> {
    let mut len_buf = [0u8; 2];
    stream.read_exact(&mut len_buf).await?;
    let len = u16::from_be_bytes(len_buf) as usize;
    
    let mut data = vec![0u8; len];
    stream.read_exact(&mut data).await?;

    match forward_to_doh(client, resolver_url, data).await {
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

async fn forward_to_doh(client: Client, resolver_url: Arc<String>, data: Vec<u8>) -> Result<Vec<u8>> {
    let resp = client
        .post(&*resolver_url)
        .header("content-type", "application/dns-message")
        .header("accept", "application/dns-message")
        .body(data)
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("Resolver status {}", resp.status()));
    }

    Ok(resp.bytes().await?.to_vec())
}
