use clap::Parser;
use std::net::{SocketAddr, IpAddr};
use anyhow::{Result, Context};
use tokio::net::{UdpSocket, TcpListener};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use reqwest::{Client, Url, Proxy};
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use tracing::{info, error, debug, Level};
use tracing_subscriber::{prelude::*};
use trust_dns_resolver::config::{ResolverConfig, NameServerConfig, ResolverOpts, Protocol};
use trust_dns_resolver::TokioAsyncResolver;
use std::time::Duration;
use std::sync::atomic::{AtomicU64, Ordering};
use nix::unistd::{User, Group, setuid, setgid};
use daemonize::Daemonize;
use std::fs::File;
use std::io::Read;

struct Stats {
    queries_udp: AtomicU64,
    queries_tcp: AtomicU64,
    errors: AtomicU64,
}

lazy_static::lazy_static! {
    static ref STATS: Stats = Stats {
        queries_udp: AtomicU64::new(0),
        queries_tcp: AtomicU64::new(0),
        errors: AtomicU64::new(0),
    };
}

#[derive(Parser, Clone)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Local IPv4/v6 address to bind to
    #[arg(short = 'a', long, default_value = "127.0.0.1")]
    listen_addr: String,

    /// Local port to bind to
    #[arg(short = 'p', long, default_value_t = 5053)]
    listen_port: u16,

    /// Number of TCP clients to serve
    #[arg(short = 'T', long, default_value_t = 20)]
    tcp_client_limit: usize,

    /// Comma-separated IPv4/v6 addresses and ports (addr:port) of DNS servers to resolve resolver host
    #[arg(short = 'b', long, default_value = "8.8.8.8,1.1.1.1,8.8.4.4,1.0.0.1,145.100.185.15,145.100.185.16,185.49.141.37")]
    bootstrap_dns: String,

    /// Optional polling interval of DNS servers
    #[arg(short = 'i', long, default_value_t = 120)]
    polling_interval: u64,

    /// Force IPv4 hostnames for DNS resolvers
    #[arg(short = '4', long)]
    force_ipv4: bool,

    /// The HTTPS path to the resolver URL
    #[arg(short = 'r', long, default_value = "https://dns.google/dns-query")]
    resolver_url: String,

    /// Optional HTTP proxy (e.g., socks5://127.0.0.1:1080)
    #[arg(short = 't', long)]
    proxy_server: Option<String>,

    /// Source IPv4/v6 address for outbound HTTPS connections
    #[arg(short = 'S', long)]
    source_addr: Option<String>,

    /// Use HTTP/1.1 instead of HTTP/2
    #[arg(short = 'x', long)]
    http11: bool,

    /// Use HTTP/3 (QUIC) only
    #[arg(short = 'q', long)]
    http3: bool,

    /// Maximum idle time in seconds allowed for reusing a HTTPS connection
    #[arg(short = 'm', long, default_value_t = 118)]
    max_idle_time: u64,

    /// Time in seconds to tolerate connection timeouts of reused connections
    #[arg(short = 'L', long, default_value_t = 15)]
    conn_loss_time: u64,

    /// Optional file containing CA certificates
    #[arg(short = 'C', long)]
    ca_path: Option<String>,

    /// Optional DSCP codepoint to set on upstream HTTPS server connections
    #[arg(short = 'c', long)]
    dscp_codepoint: Option<u8>,

    /// Daemonize
    #[arg(short = 'd', long)]
    daemonize: bool,

    /// Optional user to drop to if launched as root
    #[arg(short = 'u', long)]
    user: Option<String>,

    /// Optional group to drop to if launched as root
    #[arg(short = 'g', long)]
    group: Option<String>,

    /// Increase logging verbosity
    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Path to file to log to
    #[arg(short = 'l', long)]
    logfile: Option<String>,

    /// Optional statistic printout interval
    #[arg(short = 's', long, default_value_t = 0)]
    statistic_interval: u64,

    /// Flight recorder: storing desired amount of logs in memory
    #[arg(short = 'F', long, default_value_t = 0)]
    log_limit: usize,

    /// Print versions and exit
    #[arg(short = 'V', long)]
    print_version: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if args.print_version {
        println!("https_dns_proxy_rust {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    setup_logging(args.verbose, &args.logfile);

    if args.daemonize {
        let daemonize = Daemonize::new()
            .working_directory("/tmp")
            .umask(0o022);
        daemonize.start().context("Failed to daemonize")?;
    }

    let addr: SocketAddr = format!("{}:{}", args.listen_addr, args.listen_port)
        .parse()
        .context("Failed to parse listen address")?;

    let resolver_url_parsed = Url::parse(&args.resolver_url)
        .context("Failed to parse resolver URL")?;
    let resolver_domain = resolver_url_parsed.domain().context("Resolver URL must have a domain")?.to_string();

    // Bind sockets BEFORE dropping privileges
    let udp_socket = Arc::new(UdpSocket::bind(addr).await.context("Failed to bind UDP socket")?);
    let tcp_listener = TcpListener::bind(addr).await.context("Failed to bind TCP listener")?;

    info!("Listening on UDP/TCP {} -> {}", addr, args.resolver_url);

    // Drop privileges if requested
    if args.user.is_some() || args.group.is_some() {
        drop_privileges(&args.user, &args.group)?;
    }

    // Initial Client Setup
    let ip = resolve_bootstrap(&resolver_domain, &args.bootstrap_dns, args.force_ipv4).await?;
    info!("Bootstrapped {} to {}", resolver_domain, ip);
    let client = create_client(&args, Some((resolver_domain.clone(), ip)))?;

    let shared_client = Arc::new(RwLock::new(client));
    let resolver_url_str = Arc::new(args.resolver_url.clone());

    // Spawn Bootstrap Loop
    {
        let shared_client = shared_client.clone();
        let args = args.clone();
        let domain = resolver_domain.clone();
        let bootstrap_dns = args.bootstrap_dns.clone();
        let force_ipv4 = args.force_ipv4;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(args.polling_interval));
            loop {
                interval.tick().await;
                match resolve_bootstrap(&domain, &bootstrap_dns, force_ipv4).await {
                    Ok(new_ip) => {
                        debug!("Refreshed bootstrap IP for {}: {}", domain, new_ip);
                        match create_client(&args, Some((domain.clone(), new_ip))) {
                            Ok(new_client) => {
                                let mut w = shared_client.write().await;
                                *w = new_client;
                            }
                            Err(e) => error!("Failed to create client during bootstrap refresh: {}", e),
                        }
                    }
                    Err(e) => error!("Failed to refresh bootstrap IP: {}", e),
                }
            }
        });
    }

    // Spawn Statistics Loop
    if args.statistic_interval > 0 {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(args.statistic_interval));
            loop {
                interval.tick().await;
                let udp = STATS.queries_udp.load(Ordering::Relaxed);
                let tcp = STATS.queries_tcp.load(Ordering::Relaxed);
                let err = STATS.errors.load(Ordering::Relaxed);
                info!("Stats: UDP queries: {}, TCP queries: {}, Errors: {}", udp, tcp, err);
            }
        });
    }

    let tcp_semaphore = Arc::new(Semaphore::new(args.tcp_client_limit));

    let udp_loop = {
        let socket = udp_socket.clone();
        let shared_client = shared_client.clone();
        let resolver_url = resolver_url_str.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                match socket.recv_from(&mut buf).await {
                    Ok((len, peer)) => {
                        let data = buf[..len].to_vec();
                        let socket = socket.clone();
                        let shared_client = shared_client.clone();
                        let resolver_url = resolver_url.clone();
                        tokio::spawn(async move {
                            let client = {
                                let r = shared_client.read().await;
                                r.clone()
                            };
                            if let Err(e) = handle_udp_query(socket, client, resolver_url, data, peer).await {
                                debug!("Error handling UDP query from {}: {}", peer, e);
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
        tokio::spawn(async move {
            loop {
                match tcp_listener.accept().await {
                    Ok((mut stream, peer)) => {
                        let shared_client = shared_client.clone();
                        let resolver_url = resolver_url.clone();
                        let permit = semaphore.clone().acquire_owned().await;
                        tokio::spawn(async move {
                            let _permit = permit; // Hold permit until task finishes
                            let client = {
                                let r = shared_client.read().await;
                                r.clone()
                            };
                            if let Err(e) = handle_tcp_query(&mut stream, client, resolver_url).await {
                                debug!("Error handling TCP query from {}: {}", peer, e);
                            }
                        });
                    }
                    Err(e) => error!("TCP accept error: {}", e),
                }
            }
        })
    };

    tokio::select! {
        _ = udp_loop => {},
        _ = tcp_loop => {},
    }

    Ok(())
}

fn setup_logging(verbosity: u8, logfile: &Option<String>) {
    let level = match verbosity {
        0 => Level::INFO,
        1 => Level::DEBUG,
        _ => Level::TRACE,
    };

    let filter = tracing_subscriber::EnvFilter::from_default_env()
        .add_directive(level.into());

    let registry = tracing_subscriber::registry().with(filter);

    if let Some(path) = logfile {
        if let Ok(file) = File::create(path) {
            let layer = tracing_subscriber::fmt::layer()
                .with_writer(file)
                .with_ansi(false);
            registry.with(layer).init();
            return;
        }
    }

    let layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr);
    registry.with(layer).init();
}

fn drop_privileges(user_name: &Option<String>, group_name: &Option<String>) -> Result<()> {
    if let Some(group) = group_name {
        let g = Group::from_name(group)?
            .ok_or_else(|| anyhow::anyhow!("Group {} not found", group))?;
        setgid(g.gid).context("Failed to set gid")?;
        info!("Dropped privileges to group {}", group);
    }
    if let Some(user) = user_name {
        let u = User::from_name(user)?
            .ok_or_else(|| anyhow::anyhow!("User {} not found", user))?;
        setuid(u.uid).context("Failed to set uid")?;
        info!("Dropped privileges to user {}", user);
    }
    Ok(())
}

async fn resolve_bootstrap(domain: &str, bootstrap_dns: &str, force_ipv4: bool) -> Result<SocketAddr> {
    let servers: Vec<SocketAddr> = bootstrap_dns
        .split(',')
        .map(|s| {
            let s = s.trim();
            if let Ok(ip) = s.parse::<IpAddr>() {
                SocketAddr::new(ip, 53)
            } else {
                s.parse().unwrap_or_else(|_| panic!("Invalid bootstrap address: {}", s))
            }
        })
        .collect();

    let mut config = ResolverConfig::new();
    for s in servers {
        config.add_name_server(NameServerConfig {
            socket_addr: s,
            protocol: Protocol::Udp,
            tls_dns_name: None,
            trust_negative_responses: false,
            bind_addr: None,
        });
        config.add_name_server(NameServerConfig {
            socket_addr: s,
            protocol: Protocol::Tcp,
            tls_dns_name: None,
            trust_negative_responses: false,
            bind_addr: None,
        });
    }

    let mut opts = ResolverOpts::default();
    if force_ipv4 {
        opts.ip_strategy = trust_dns_resolver::config::LookupIpStrategy::Ipv4Only;
    }

    let resolver = TokioAsyncResolver::tokio(config, opts);
    let ips = resolver.lookup_ip(domain).await.context("Failed to resolve DoH provider hostname")?;
    
    let ip = if force_ipv4 {
        ips.iter().find(|ip| ip.is_ipv4()).ok_or_else(|| anyhow::anyhow!("No IPv4 address found"))?
    } else {
        ips.iter().next().ok_or_else(|| anyhow::anyhow!("No IPs returned"))?
    };
    
    Ok(SocketAddr::new(ip, 443))
}

fn create_client(args: &Args, resolve_override: Option<(String, SocketAddr)>) -> Result<Client> {
    let mut builder = Client::builder()
        .use_rustls_tls()
        .pool_idle_timeout(Duration::from_secs(args.max_idle_time))
        .connect_timeout(Duration::from_secs(args.conn_loss_time));

    if args.http11 {
        builder = builder.http1_only();
    } else if args.http3 {
        builder = builder.http3_prior_knowledge();
    }

    if let Some(proxy_url) = &args.proxy_server {
        let proxy = Proxy::all(proxy_url).context("Failed to parse proxy URL")?;
        builder = builder.proxy(proxy);
    }

    if let Some(source_addr) = &args.source_addr {
        let ip = source_addr.parse::<IpAddr>().context("Failed to parse source address")?;
        builder = builder.local_address(ip);
    }

    if let Some(ca_path) = &args.ca_path {
        let mut buf = Vec::new();
        File::open(ca_path)?.read_to_end(&mut buf)?;
        let cert = reqwest::Certificate::from_pem(&buf)?;
        builder = builder.add_root_certificate(cert);
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
) -> Result<()> {
    STATS.queries_udp.fetch_add(1, Ordering::Relaxed);
    debug!("UDP query from {}", peer);
    match forward_to_doh(client, resolver_url, data).await {
        Ok(bytes) => {
            socket.send_to(&bytes, peer).await?;
            Ok(())
        }
        Err(e) => {
            STATS.errors.fetch_add(1, Ordering::Relaxed);
            Err(e)
        }
    }
}

async fn handle_tcp_query(
    stream: &mut tokio::net::TcpStream,
    client: Client,
    resolver_url: Arc<String>,
) -> Result<()> {
    STATS.queries_tcp.fetch_add(1, Ordering::Relaxed);
    let mut len_buf = [0u8; 2];
    stream.read_exact(&mut len_buf).await?;
    let len = u16::from_be_bytes(len_buf) as usize;
    
    let mut data = vec![0u8; len];
    stream.read_exact(&mut data).await?;

    debug!("TCP query of length {}", len);
    match forward_to_doh(client, resolver_url, data).await {
        Ok(bytes) => {
            let resp_len = (bytes.len() as u16).to_be_bytes();
            stream.write_all(&resp_len).await?;
            stream.write_all(&bytes).await?;
            Ok(())
        }
        Err(e) => {
            STATS.errors.fetch_add(1, Ordering::Relaxed);
            Err(e)
        }
    }
}

async fn forward_to_doh(
    client: Client,
    resolver_url: Arc<String>,
    data: Vec<u8>,
) -> Result<Vec<u8>> {
    let resp = client
        .post(&*resolver_url)
        .header("content-type", "application/dns-message")
        .header("accept", "application/dns-message")
        .body(data)
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("Resolver returned status {}", resp.status()));
    }

    Ok(resp.bytes().await?.to_vec())
}
