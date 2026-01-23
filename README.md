# https_dns_proxy_rust

A lightweight, asynchronous DNS-to-HTTPS (DoH) proxy written in Rust.

## Features

- **UDP & TCP Support**: Listen for DNS queries on both protocols.
- **Asynchronous**: Built with [Tokio](https://tokio.rs/) for high performance.
- **DoH Forwarding**: Uses [reqwest](https://docs.rs/reqwest/) to tunnel queries over HTTPS (RFC 8484).

## Usage

### Build

```bash
cargo build --release
```

### Run

```bash
./target/release/https_dns_proxy_rust --help

# Example: Listen on port 5053, use Google DoH
./target/release/https_dns_proxy_rust -p 5053 -r https://dns.google/dns-query
```

## Configuration

| Flag | Description | Default |
|------|-------------|---------|
| `-a`, `--listen-addr` | Address to bind to | 127.0.0.1 |
| `-p`, `--listen-port` | Port to bind to | 5053 |
| `-r`, `--resolver-url` | DoH Resolver URL | https://dns.google/dns-query |
