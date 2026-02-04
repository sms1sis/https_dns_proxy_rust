# SafeDNS (https_dns_proxy_rust)

**SafeDNS** is a lightweight, non-caching DNS-to-HTTPS translation proxy written in Rust. It serves as a modern, memory-safe alternative to the original C-based [https_dns_proxy](https://github.com/aarond10/https_dns_proxy), compliant with RFC 8484 (DNS-over-HTTPS).

## Project Overview

*   **Language:** Rust (2024 edition)
*   **Runtime:** Tokio (Async I/O)
*   **Android App:** Kotlin + Jetpack Compose (Material 3)
*   **Package Name:** `io.github.SafeDNS`

## Recent Android Updates (v0.3.2)

*   **Rebranding:** Fully rebranded as **SafeDNS** with a new package identity and polished UI.
*   **Persistent HTTP Connections:** Implemented a `DynamicResolver` to maintain a single `reqwest::Client` with a shared connection pool, eliminating latency spikes during bootstrap refreshes.
*   **Smart DNS Caching:** Added TTL extraction from DNS response headers (via `hickory-proto`) to ensure cache freshness and protocol compliance.
*   **Network Robustness:** Introduced a retry mechanism for DoH requests with exponential backoff to mitigate transient HTTP/Network errors.
*   **High-Concurrency Tuning:** Optimized connection pooling (32 idle connections per host) for better multi-app performance.
*   **Native TLS Verification:** Integrated `rustls-platform-verifier` via JNI to use the Android system's native CA trust store.
*   **Retry Reliability:** Implemented a retry loop (5 attempts) for socket binding to prevent "Address already in use" errors during rapid proxy restarts.

## Directory Structure

*   `src/`: Rust core and JNI bindings.
*   `android/`: Android Studio project.
    *   `app/src/main/java/io/github/SafeDNS/`: Kotlin UI and VpnService logic.
*   `build_android.sh`: Orchestrates the cross-compilation and JNI library collection.

## Android Implementation Notes

*   **Architecture:** Optimized for `arm64-v8a` (ARMv8-A + NEON).
*   **Privileged Ports:** App binds the Rust proxy to port `5053` to bypass Android's non-root restriction on port `53`.
*   **Split Tunneling:** Uses `addRoute("10.0.0.2", 32)` and `allowBypass()` to ensure the VPN only handles DNS traffic, preventing infinite loops and maintaining global internet access.
*   **SSL Roots:** Bundles `webpki-roots` in the Rust binary to fix `UnknownIssuer` SSL errors common on Android environments.

## Build Commands (Android)

```bash
# Cross-compile Rust for arm64-v8a and update JNI libs
./build_android.sh

# Build and Install the Android APK
cd android && ./gradlew installDebug
```