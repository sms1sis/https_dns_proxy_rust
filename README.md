# SafeDNS (https_dns_proxy_rust)

A lightweight, asynchronous DNS-over-HTTPS (DoH) proxy written in Rust, featuring a premium Android application.

This project is a modern, memory-safe Rust implementation of the original C-based https-dns-proxy, fully compliant with RFC 8484.

## 📱 SafeDNS Android (Pro v0.2.0)

SafeDNS includes a feature-rich Android app (arm64-v8a) that provides an encrypted tunnel for all your DNS traffic without requiring root.

### Features
- **One-Tap Connection:** Merged status & power button for instant protection.
- **Premium UI:** Modern, compact dashboard with dynamic pulsating animations.
- **AMOLED Theme:** Pure black theme for battery savings on OLED screens.
- **DNS Profiles:** Pre-configured support for Cloudflare, Google, AdGuard, and Quad9.
- **Real-time Metrics:** Live latency (ms) tracking with a configurable heartbeat interval.
- **Activity Logs:** Chronological DNS query logs with auto-scroll and export-to-file capability.
- **Smart Routing:** Split-tunneling via `VpnService` to ensure global internet stability.
- **Auto-Start:** Automatically resume protection after device reboot.
- **Quick Settings:** Toggle DNS protection directly from the Android notification shade.

## 💻 CLI Usage (Rust Core)

### Build
```bash
cargo build --release
```

### Build for Android (Local)
Requires Android NDK and `cargo-ndk`.
```bash
./build_android.sh
```

### Run (CLI)
```bash
./target/release/https_dns_proxy_rust -p 5053 -r https://cloudflare-dns.com/dns-query
```

## 🛠 Configuration (CLI)

| Flag | Description | Default |
|------|-------------|---------|
| `-a`, `--listen-addr` | Address to bind to | 127.0.0.1 |
| `-p`, `--listen-port` | Port to bind to | 5053 |
| `-r`, `--resolver-url` | DoH Resolver URL | https://cloudflare-dns.com/dns-query |
| `-b`, `--bootstrap-dns` | Bootstrap DNS IPs | 1.1.1.1 |

---
Created by **sms1sis**. Project repo: [https_dns_proxy_rust](https://github.com/sms1sis/https_dns_proxy_rust/tree/android-app)