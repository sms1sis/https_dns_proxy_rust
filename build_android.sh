#!/bin/bash

# Exit on error
set -e

# Define targets
TARGETS=("aarch64-linux-android" "armv7-linux-androideabi" "i686-linux-android" "x86_64-linux-android")

# Check for RUSTFLAGS
export RUSTFLAGS="--cfg reqwest_unstable"

echo "Building for Android targets..."

for TARGET in "${TARGETS[@]}"; do
    echo "--------------------------------------------------"
    echo "Building for $TARGET..."
    
    # Check if target is installed
    if ! rustup target list | grep -q "$TARGET (installed)"; then
        echo "Target $TARGET not found. Installing..."
        rustup target add "$TARGET"
    fi

    cargo build --release --target "$TARGET" --features jni
done

echo "--------------------------------------------------"
echo "Build complete! Libraries are in target/<target>/release/libhttps_dns_proxy_rust.so"

# Create a directory to collect all libraries
OUTPUT_DIR="android_libs"
mkdir -p "$OUTPUT_DIR/arm64-v8a"
mkdir -p "$OUTPUT_DIR/armeabi-v7a"
mkdir -p "$OUTPUT_DIR/x86"
mkdir -p "$OUTPUT_DIR/x86_64"

cp target/aarch64-linux-android/release/libhttps_dns_proxy_rust.so "$OUTPUT_DIR/arm64-v8a/"
cp target/armv7-linux-androideabi/release/libhttps_dns_proxy_rust.so "$OUTPUT_DIR/armeabi-v7a/"
cp target/i686-linux-android/release/libhttps_dns_proxy_rust.so "$OUTPUT_DIR/x86/"
cp target/x86_64-linux-android/release/libhttps_dns_proxy_rust.so "$OUTPUT_DIR/x86_64/"

echo "All libraries collected in $OUTPUT_DIR/"
