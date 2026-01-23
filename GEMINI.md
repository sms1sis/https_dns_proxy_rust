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
*   `-b`, `--bootstrap-dns <IPs>`: Comma-separated list of bootstrap DNS servers (IPs) to resolve the DoH provider's hostname.
*   `-r`, `--resolver-url <URL>`: Resolver URL (DoH endpoint) (default: `https://dns.google/dns-query`).
*   `-v`: Increase verbosity (e.g., `-v`, `-vv` for debug/trace logs).
*   `-h`, `--help`: Print help.

## Features vs C Version

*   **Memory Safety:** Built with Rust to prevent common memory vulnerabilities.
*   **Async/Await:** Uses Tokio for efficient handling of concurrent UDP and TCP connections.
*   **Bootstrap DNS:** Built-in support for bootstrapping the DoH provider IP using standard DNS (via `trust-dns-resolver`), ensuring reliability even if the system DNS is unavailable.