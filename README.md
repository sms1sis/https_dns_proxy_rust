# https_dns_proxy_rust

A lightweight, asynchronous DNS-to-HTTPS (DoH) proxy written in Rust.

This project is a Rust implementation of the [original C-based https-dns-proxy](https://github.com/stangri/https-dns-proxy) by [stangri](https://github.com/stangri).

## Features

- **UDP & TCP Support**: Listen for DNS queries on both protocols.
- **Asynchronous**: Built with [Tokio](https://tokio.rs/) for high performance.
- **DoH Forwarding**: Uses [reqwest](https://docs.rs/reqwest/) to tunnel queries over HTTPS (RFC 8484).

## Usage

### Build

```bash
cargo build --release
```

### Cross-Compilation (Advanced)

Use the unified management script to setup environments and build for multiple architectures (statically linked with musl).

#### Prerequisites for `manage.sh`
Before running the script on a fresh Linux environment, ensure you have the following installed:
- **OS:** x86_64 Linux host
- **Tools:** `curl`, `tar`, `git`, and `build-essential` (or equivalent development tools)

Example for Ubuntu/Debian:
```bash
sudo apt update && sudo apt install build-essential curl git
```

#### Usage
```bash
# Setup environment (downloads toolchains, configures rustup locally)
./manage.sh setup all       # Setup for all architectures (arm64)

# Build binaries
./manage.sh build all       # Build all architectures (arm64)
./manage.sh build arm64     # Build only for arm64
```

Supported architectures: `arm64`.

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
