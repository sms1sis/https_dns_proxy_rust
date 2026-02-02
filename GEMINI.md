# https_dns_proxy_rust

**https_dns_proxy_rust** is a lightweight, non-caching DNS-to-HTTPS translation proxy written in Rust. It serves as a modern, memory-safe alternative to the original C-based [https_dns_proxy](https://github.com/aarond10/https_dns_proxy), compliant with RFC 8484 (DNS-over-HTTPS).

## Project Overview

*   **Language:** Rust (2021 edition)
*   **Runtime:** Tokio (Async I/O)
*   **Android App:** Kotlin + Jetpack Compose (Material 3)

## Recent Android Updates (v0.2.0)

*   **JNI Integration:** The Rust proxy is now built as a shared library (`.so`) and called via JNI from the Android `ProxyService`.
*   **VpnService Engine:** Implemented a manual IPv4/UDP packet forwarder that intercepts DNS traffic and tunnels it through the Rust DoH proxy.
*   **Latency Heartbeat:** A background thread performs periodic DNS queries to provide real-time latency (ms) metrics.
*   **Chronological Logging:** Rust backend logs are exposed via JNI to a live, auto-scrolling activity feed in the UI.
*   **AMOLED Theme:** Pure black background support for OLED efficiency.
*   **Persistence:** All settings are stored in `SharedPreferences` for continuity across restarts.

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