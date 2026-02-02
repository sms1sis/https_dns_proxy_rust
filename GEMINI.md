# https_dns_proxy_rust

**https_dns_proxy_rust** is a lightweight, non-caching DNS-to-HTTPS translation proxy written in Rust. It serves as a modern, memory-safe alternative to the original C-based [https_dns_proxy](https://github.com/aarond10/https_dns_proxy), compliant with RFC 8484 (DNS-over-HTTPS).

It accepts standard DNS requests (UDP/TCP) and resolves them via a DoH provider (e.g., Google, Cloudflare).

## Project Overview

*   **Language:** Rust (2021 edition)
*   **Runtime:** Tokio (Async I/O)
*   **Key Dependencies:**
    *   `tokio`: Asynchronous runtime for non-blocking I/O (UDP/TCP).
    *   `reqwest`: HTTP client for DoH requests (using `rustls`).
    *   `trust-dns-resolver`: For bootstrapping the DoH provider IP.
    *   `clap`: Command-line argument parsing.
    *   `tracing`: Structured logging.

## Directory Structure

*   `src/`: Rust source files.
    *   `main.rs`: Application entry point, CLI parsing, and DNS forwarding logic.
*   `Cargo.toml`: Project configuration and dependencies.

## Building

### Prerequisites

*   **Toolchain:** Rust (cargo, rustc)

### Build Commands

```bash
# Build for development
cargo build

# Build for release
cargo build --release
```

## Running

The proxy listens for DNS queries on a local port and forwards them to a DoH provider.

**Basic Usage:**
```bash
cargo run -- -b 8.8.8.8 -r https://dns.google/dns-query
```

**Binary Usage:**
```bash
./target/release/https_dns_proxy_rust --bootstrap-dns 1.1.1.1 --resolver-url https://cloudflare-dns.com/dns-query
```

**Common Flags:**
*   `-a`, `--listen-addr <IP>`: Listen address (default: `127.0.0.1`).
*   `-p`, `--listen-port <PORT>`: Listen port (default: `5053`).
*   `-T`, `--tcp-client-limit <N>`: Max concurrent TCP clients (default: `20`).
*   `-b`, `--bootstrap-dns <IPs>`: Comma-separated list of bootstrap DNS servers.
*   `-i`, `--polling-interval <SECS>`: Interval to refresh DoH provider IP (default: `120`).
*   `-4`, `--force-ipv4`: Force IPv4 for DoH provider resolution.
*   `-r`, `--resolver-url <URL>`: DoH endpoint (default: `https://dns.google/dns-query`).
*   `-t`, `--proxy-server <URL>`: HTTP/SOCKS5 proxy.
*   `-S`, `--source-addr <IP>`: Source IP for outbound connections.
*   `-x`, `--http11`: Force HTTP/1.1.
*   `-q`, `--http3`: Force HTTP/3 (QUIC).
*   `-m`, `--max-idle-time <SECS>`: Max idle time for HTTPS connections (default: `118`).
*   `-L`, `--conn-loss-time <SECS>`: Connection timeout (default: `15`).
*   `-C`, `--ca-path <FILE>`: Path to CA certificates.
*   `-d`, `--daemonize`: Run in background.
*   `-u`, `--user <USER>`: Drop privileges to user.
*   `-g`, `--group <GROUP>`: Drop privileges to group.
*   `-v`: Increase verbosity.
*   `-l`, `--logfile <FILE>`: Log to file.
*   `-s`, `--statistic-interval <SECS>`: Interval to print stats.
*   `-V`, `--print-version`: Print version and exit.
*   `-h`, `--help`: Print help.

## Android Specific Notes



*   **Architecture:** Optimized for `arm64-v8a` (ARMv8-A).

*   **Privileged Ports:** Android prevents binding to ports < 1024 (like 53) for non-root apps. Always use a high port (e.g., 5053) for the Rust proxy.

*   **VPN DNS Routing:** Using `127.0.0.1` as a DNS server in `VpnService.Builder` causes `IllegalArgumentException: Bad address` on many Android versions. Use the VPN interface IP (e.g., `10.0.0.2`) instead.

*   **Connectivity:** To prevent the "no internet" issue, only route the DNS server's IP (e.g., `.addRoute("10.0.0.2", 32)`) into the VPN. Routing `0.0.0.0/0` will drop all non-DNS traffic unless the app is specifically designed as a full-tunnel VPN.

*   **Service Lifecycle:** Use explicit "STOP" actions and ensure `vpnInterface.close()` is called to properly shut down the VPN and clear the status bar icon.
