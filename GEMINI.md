# https_dns_proxy_rust

**https_dns_proxy_rust** is a lightweight, non-caching DNS-to-HTTPS translation proxy written in Rust. It serves as a modern, memory-safe alternative to the original C-based [https_dns_proxy](https://github.com/aarond10/https_dns_proxy), compliant with RFC 8484 (DNS-over-HTTPS).

## Project Overview

*   **Language:** Rust (2021 edition)
*   **Runtime:** Tokio (Async I/O)
*   **Android App:** Kotlin + Jetpack Compose (Material 3)

## Recent Android Updates (v0.3.1)

*   **Rust 2024 Edition:** Core engine upgraded to Rust Edition 2024 using `std::sync::LazyLock` and modern asynchronous patterns.
*   **IPv6 Support:** Implemented mandatory IPv6 UDP checksum calculation using the IPv6 pseudo-header for full DNS interception support.
*   **Native TLS Verification:** Integrated `rustls-platform-verifier` via JNI to use the Android system's native CA trust store, fixing SSL connectivity issues.
*   **DNS Caching:** Added a high-performance `moka` cache with user-configurable TTL and a manual "Clear Cache" UI action.
*   **Dynamic Builds:** Replaced hardcoded paths in `settings.gradle` with dynamic `cargo metadata` lookups for portable cross-compilation.
*   **Bind Reliability:** Implemented a retry loop (5 attempts) for socket binding to prevent "Address already in use" errors during rapid proxy restarts.

## Directory Structure

*   `src/`: Rust core and JNI bindings.
*   `android/`: Android Studio project.
    *   `app/src/main/java/`: Kotlin UI and VpnService logic.
*   `build_android.sh`: Orchestrates the cross-compilation and JNI library collection.

## Android Implementation Notes

*   **Architecture:** Optimized for `arm64-v8a` (ARMv8-A + NEON).
*   **Privileged Ports:** App binds the Rust proxy to port `5053` to bypass Android's non-root restriction on port `53`.
*   **Split Tunneling:** Uses `addRoute("1.1.1.1", 32)` and `allowBypass()` to ensure the VPN only handles DNS traffic, preventing infinite loops and maintaining global internet access.
*   **SSL Roots:** Bundles `webpki-roots` in the Rust binary to fix `UnknownIssuer` SSL errors common on Android environments.

## Build Commands (Android)

```bash
# Cross-compile Rust for arm64-v8a and update JNI libs
./build_android.sh

# Build the Android APK
cd android && ./gradlew assembleDebug
```