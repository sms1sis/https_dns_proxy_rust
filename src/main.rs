use clap::Parser;
use std::net::SocketAddr;
use anyhow::Result;
use tokio::net::{UdpSocket, TcpListener};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use reqwest::Client;
use std::sync::Arc;
use tracing::{info, error, debug};

#[derive(Parser, Clone)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value = "127.0.0.1")]
    listen_addr: String,

    #[arg(short, long, default_value_t = 5053)]
    listen_port: u16,

    #[arg(short, long, default_value = "https://dns.google/dns-query")]
    resolver_url: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let addr: SocketAddr = format!("{}:{}", args.listen_addr, args.listen_port).parse()?;
    let udp_socket = Arc::new(UdpSocket::bind(addr).await?);
    let tcp_listener = TcpListener::bind(addr).await?;
    let client = Client::new();
    let resolver_url = Arc::new(args.resolver_url.clone());

    info!("Listening on UDP/TCP {}", addr);

    let udp_loop = {
        let socket = udp_socket.clone();
        let client = client.clone();
        let resolver_url = resolver_url.clone();
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
                                error!("Error handling UDP query from {}: {}", peer, e);
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
        let resolver_url = resolver_url.clone();
        tokio::spawn(async move {
            loop {
                match tcp_listener.accept().await {
                    Ok((mut stream, peer)) => {
                        let client = client.clone();
                        let resolver_url = resolver_url.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_tcp_query(&mut stream, client, resolver_url).await {
                                error!("Error handling TCP query from {}: {}", peer, e);
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
        .body(data)
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("Resolver returned status {}", resp.status()));
    }

    Ok(resp.bytes().await?.to_vec())
}
