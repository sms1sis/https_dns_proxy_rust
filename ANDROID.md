# Android Integration Guide

This branch (`android-app`) contains changes to make `https_dns_proxy_rust` usable as a library within an Android application.

## Key Changes

1.  **Library/Binary Split**: The core logic is now in `src/lib.rs`.
2.  **JNI Bindings**: Added JNI exports in `src/lib.rs` under the `jni` feature.
3.  **Graceful Shutdown**: The `run_proxy` function now accepts a shutdown receiver.
4.  **Android Logging**: Integration with `android_logger`.

## Building for Android

To build the shared library for Android, you'll need the Android NDK installed and the appropriate Rust targets.

```bash
# Add targets
rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android

# Build for aarch64 (64-bit ARM)
cargo build --release --target aarch64-linux-android --features jni
```

The resulting library will be at `target/aarch64-linux-android/release/libhttps_dns_proxy_rust.so`.

## Usage in Android (Kotlin)

1.  **Copy the `.so` file**: Place the compiled library in your Android project's `src/main/jniLibs/<abi>/` directory.
2.  **Define the Service**:

```kotlin
class ProxyService : Service() {
    companion object {
        init {
            System.loadLibrary("https_dns_proxy_rust")
        }
    }

    external fun initLogger()
    external fun startProxy(listenAddr: String, listenPort: Int, resolverUrl: String, bootstrapDns: String): Int
    external fun stopProxy()

    override fun onCreate() {
        super.onCreate()
        initLogger()
        // Run in a background thread or Coroutine
        thread {
            startProxy("127.0.0.1", 5053, "https://dns.google/dns-query", "8.8.8.8,1.1.1.1")
        }
    }

    override fun onDestroy() {
        stopProxy()
        super.onDestroy()
    }
}
```

## Note on Package Name

The JNI functions are currently named for the package `com.example.httpsdnsproxy`. If your app uses a different package name, you **must** update the function names in `src/lib.rs`.

Example for `com.mycompany.dns`:
`Java_com_mycompany_dns_ProxyService_startProxy`
