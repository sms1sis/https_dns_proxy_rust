#!/bin/bash
set -e

# 1. Point to your specific NDK version
export ANDROID_NDK_HOME=$HOME/Android/Sdk/ndk/29.0.14206865

# 2. Add the NDK toolchain to your PATH so Cargo can find the 'clang' tools
TOOLCHAIN=$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin
export PATH=$TOOLCHAIN:$PATH

# Define targets
TARGETS=("aarch64-linux-android")
export RUSTFLAGS="--cfg reqwest_unstable"

echo "Using NDK from: $ANDROID_NDK_HOME"

for TARGET in "${TARGETS[@]}"; do
    echo "--------------------------------------------------"
    echo "Building for $TARGET..."

    # Use --platform instead of -p to avoid confusion with --package
    cargo ndk --target "$TARGET" --platform 21 build --release --lib --features jni
done

echo "--------------------------------------------------"
echo "Build complete! Collecting libraries..."

# Copy to the Android project's jniLibs directory
OUTPUT_DIR="android/app/src/main/jniLibs"
mkdir -p "$OUTPUT_DIR/arm64-v8a"

cp target/aarch64-linux-android/release/libhttps_dns_proxy_rust.so "$OUTPUT_DIR/arm64-v8a/"

echo "All libraries collected in $OUTPUT_DIR/"
