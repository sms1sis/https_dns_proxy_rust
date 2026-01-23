use clap::Parser;
use std::net::{SocketAddr, IpAddr};
use anyhow::{Result, Context};
use tokio::net::{UdpSocket, TcpListener};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use reqwest::{Client, Url, Proxy};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, error, debug, Level};
use tracing_subscriber::FmtSubscriber;
use trust_dns_resolver::config::{ResolverConfig, NameServerConfig, ResolverOpts, Protocol};
use trust_dns_resolver::TokioAsyncResolver;
use std::time::Duration;
use nix::unistd::{User, Group, setuid, setgid};

#[derive(Parser, Clone)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Listen address
    #[arg(short = 'a', long, default_value = "127.0.0.1")]
    listen_addr: String,

    /// Listen port
    #[arg(short = 'p', long, default_value_t = 5053)]
    listen_port: u16,

    /// Resolver URL (DoH endpoint)
    #[arg(short, long, default_value = "https://dns.google/dns-query")]
    resolver_url: String,

    /// Bootstrap DNS servers (comma-separated IPs)
    #[arg(short = 'b', long)]
    bootstrap_dns: Option<String>,

    /// Bootstrap DNS polling interval in seconds
    #[arg(long, default_value_t = 120)]
    bootstrap_interval: u64,

    /// Proxy URL (e.g., socks5://127.0.0.1:9050 or http://127.0.0.1:8080)
    #[arg(long)]
    proxy_url: Option<String>,

    /// Connection timeout in seconds
    #[arg(long, default_value_t = 10)]
    connect_timeout: u64,

    /// Idle timeout in seconds (keep-alive)
    #[arg(long, default_value_t = 90)]
    idle_timeout: u64,

    /// Drop privileges to this user
    #[arg(short = 'u', long)]
    user: Option<String>,

    /// Drop privileges to this group
    #[arg(short = 'g', long)]
    group: Option<String>,

    /// Verbosity level (-v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    setup_logging(args.verbose);

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
    let client = if let Some(bootstrap_dns) = &args.bootstrap_dns {
        let ip = resolve_bootstrap(&resolver_domain, bootstrap_dns).await?;
        info!("Bootstrapped {} to {}", resolver_domain, ip);
        create_client(&args, Some((resolver_domain.clone(), ip)))?
    } else {
        create_client(&args, None)?
    };

    let shared_client = Arc::new(RwLock::new(client));
    let resolver_url_str = Arc::new(args.resolver_url.clone());

    // Spawn Bootstrap Loop if needed
    if let Some(bootstrap_dns) = args.bootstrap_dns.clone() {
        let shared_client = shared_client.clone();
        let args = args.clone();
        let domain = resolver_domain.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(args.bootstrap_interval));
            loop {
                interval.tick().await;
                match resolve_bootstrap(&domain, &bootstrap_dns).await {
                    Ok(new_ip) => {
                        debug!("Refreshed bootstrap IP for {}: {}", domain, new_ip);
                        // We could optimize by checking if IP changed, but creating a client is relatively cheap occasionally.
                        // Actually, checking is better to preserve connection pools.
                        // For simplicity/robustness of this snippet, we'll just rebuild if we want strict correctness, 
                        // but let's check if we can easily peek. 
                        // Since we can't easily peek inside the client's resolve map, we'll unconditionally update 
                        // or maybe store the last IP in this loop?
                        // Let's store last_ip.
                        // Note: To really do this right we'd need to track state. 
                        // For now, let's just rebuild.
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
                            // Acquire read lock to get the current client
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
        tokio::spawn(async move {
            loop {
                match tcp_listener.accept().await {
                    Ok((mut stream, peer)) => {
                        let shared_client = shared_client.clone();
                        let resolver_url = resolver_url.clone();
                        tokio::spawn(async move {
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

fn setup_logging(verbosity: u8) {
    let level = match verbosity {
        0 => Level::INFO,
        1 => Level::DEBUG,
        _ => Level::TRACE,
    };

    let subscriber = FmtSubscriber::builder()
        .with_max_level(level)
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .expect("setting default subscriber failed");
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

async fn resolve_bootstrap(domain: &str, bootstrap_dns: &str) -> Result<SocketAddr> {
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
        // Also add TCP
        config.add_name_server(NameServerConfig {
            socket_addr: s,
            protocol: Protocol::Tcp,
            tls_dns_name: None,
            trust_negative_responses: false,
            bind_addr: None,
        });
    }

    let resolver = TokioAsyncResolver::tokio(config, ResolverOpts::default());
    let ips = resolver.lookup_ip(domain).await.context("Failed to resolve DoH provider hostname")?;
    
    // Naively pick the first one. A better implementation might round-robin or test them.
    let ip = ips.iter().next().ok_or_else(|| anyhow::anyhow!("No IPs returned"))?;
    
    Ok(SocketAddr::new(ip, 443))
}

fn create_client(args: &Args, resolve_override: Option<(String, SocketAddr)>) -> Result<Client> {
    let mut builder = Client::builder()
        .use_rustls_tls()
        .connect_timeout(Duration::from_secs(args.connect_timeout))
        .pool_idle_timeout(Duration::from_secs(args.idle_timeout));

    if let Some(proxy_url) = &args.proxy_url {
        let proxy = Proxy::all(proxy_url).context("Failed to parse proxy URL")?;
        builder = builder.proxy(proxy);
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
    debug!("UDP query from {}", peer);
    let bytes = forward_to_doh(client, resolver_url, data).await?;
    socket.send_to(&bytes, peer).await?;
    Ok(())
}

async fn handle_tcp_query(
    stream: &mut tokio::net::TcpStream,
    client: Client,
    resolver_url: Arc<String>,
) -> Result<()> {
    let mut len_buf = [0u8; 2];
    stream.read_exact(&mut len_buf).await?;
    let len = u16::from_be_bytes(len_buf) as usize;
    
    let mut data = vec![0u8; len];
    stream.read_exact(&mut data).await?;

    debug!("TCP query of length {}", len);
    let bytes = forward_to_doh(client, resolver_url, data).await?;
    
    let resp_len = (bytes.len() as u16).to_be_bytes();
    stream.write_all(&resp_len).await?;
    stream.write_all(&bytes).await?;
    
    Ok(())
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
