use clap::Parser;
use std::net::{SocketAddr, IpAddr};
use anyhow::{Result, Context};
use tokio::net::{UdpSocket, TcpListener};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use reqwest::{Client, Url};
use std::sync::Arc;
use tracing::{info, error, debug, Level};
use tracing_subscriber::FmtSubscriber;
use trust_dns_resolver::config::{ResolverConfig, NameServerConfig, ResolverOpts, Protocol};
use trust_dns_resolver::TokioAsyncResolver;

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
    
    let client = create_client(&args, &resolver_url_parsed).await?;
    let resolver_url_str = Arc::new(args.resolver_url.clone());

    let udp_socket = Arc::new(UdpSocket::bind(addr).await.context("Failed to bind UDP socket")?);
    let tcp_listener = TcpListener::bind(addr).await.context("Failed to bind TCP listener")?;

    info!("Listening on UDP/TCP {} -> {}", addr, args.resolver_url);

    let udp_loop = {
        let socket = udp_socket.clone();
        let client = client.clone();
        let resolver_url = resolver_url_str.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                match socket.recv_from(&mut buf).await {
                    Ok((len, peer)) => {
                        let data = buf[..len].to_vec();
                        let socket = socket.clone();
                        let client = client.clone();
                        let resolver_url = resolver_url.clone();
                        tokio::spawn(async move {
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
        let client = client.clone();
        let resolver_url = resolver_url_str.clone();
        tokio::spawn(async move {
            loop {
                match tcp_listener.accept().await {
                    Ok((mut stream, peer)) => {
                        let client = client.clone();
                        let resolver_url = resolver_url.clone();
                        tokio::spawn(async move {
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

async fn create_client(args: &Args, url: &Url) -> Result<Client> {
    let mut client_builder = Client::builder()
        .use_rustls_tls();

    if let Some(bootstrap_servers) = &args.bootstrap_dns {
        if let Some(domain) = url.domain() {
            info!("Bootstrapping {} using servers: {}", domain, bootstrap_servers);
            
            let servers: Vec<SocketAddr> = bootstrap_servers
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

            let resolver = TokioAsyncResolver::tokio(config, ResolverOpts::default());
            
            let ips = resolver.lookup_ip(domain).await.context("Failed to resolve DoH provider hostname using bootstrap servers")?;
            
            let addrs: Vec<SocketAddr> = ips.iter().map(|ip| SocketAddr::new(ip, 443)).collect();
            
            if addrs.is_empty() {
                return Err(anyhow::anyhow!("No IPs resolved for bootstrap domain"));
            }

            info!("Resolved {} to {:?}", domain, addrs);
            client_builder = client_builder.resolve(domain, addrs[0]);
        }
    }

    Ok(client_builder.build()?)
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
