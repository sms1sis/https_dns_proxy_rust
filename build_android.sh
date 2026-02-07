#!/bin/bash
set -e

# 1. Point to your specific NDK version
if [ -z "$ANDROID_NDK_HOME" ]; then
    export ANDROID_NDK_HOME=$HOME/Android/Sdk/ndk/29.0.14206865
fi

# 2. Add the NDK toolchain to your PATH so Cargo can find the 'clang' tools
TOOLCHAIN=$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin
export PATH=$TOOLCHAIN:$PATH

# Define target - focusing ONLY on arm64-v8a (ARMv8-A)
TARGET="aarch64-linux-android"

# Optimization flags for ARMv8-A
export RUSTFLAGS="-C target-cpu=generic -C target-feature=+neon --cfg reqwest_unstable"

echo "Using NDK from: $ANDROID_NDK_HOME"
echo "Building for ARMv8-A (arm64-v8a)..."

# Ensure cargo-ndk is installed
if ! command -v cargo-ndk &> /dev/null; then
    echo "cargo-ndk not found, installing..."
    cargo install cargo-ndk
fi

echo "--------------------------------------------------"
echo "Building for $TARGET..."

# Build with JNI feature
cargo ndk --target "$TARGET" --platform 26 build --release --lib --features jni

echo "--------------------------------------------------"
echo "Build complete! Collecting libraries..."

# Copy to the Android project's jniLibs directory
OUTPUT_DIR="android/app/src/main/jniLibs"
mkdir -p "$OUTPUT_DIR/arm64-v8a"

cp target/$TARGET/release/libhttps_dns_proxy_rust.so "$OUTPUT_DIR/arm64-v8a/"

echo "All libraries collected in $OUTPUT_DIR/"