#!/bin/bash
set -e

# Configuration
TOOLCHAIN_DIR="$(pwd)/toolchains"
RUSTUP_DIR="$(pwd)/local-rust"
export RUSTUP_HOME="$RUSTUP_DIR/rustup"
export CARGO_HOME="$RUSTUP_DIR/cargo"
export PATH="$CARGO_HOME/bin:$PATH"

# Architecture Mapping: "ID|RustTarget|ToolchainName|URLSuffix"
# ID is the short name used in arguments (e.g., mips)
ARCH_MAP=(
    "x86_64|x86_64-unknown-linux-musl|x86_64-linux-musl|x86_64-linux-musl-cross.tgz"
    "x86|i686-unknown-linux-musl|i686-linux-musl|i686-linux-musl-cross.tgz"
    "arm64|aarch64-unknown-linux-musl|aarch64-linux-musl|aarch64-linux-musl-cross.tgz"
    "armv7|armv7-unknown-linux-musleabihf|armv7l-linux-musleabihf|armv7l-linux-musleabihf-cross.tgz"
    "mips|mips-unknown-linux-musl|mips-linux-musl|mips-linux-musl-cross.tgz"
)

usage() {
    echo "Usage: $0 [setup|build] [arch|all]"
    echo "Architectures: x86_64, x86, arm64, armv7, mips"
    exit 1
}

setup_rust() {
    if [ ! -d "$RUSTUP_DIR" ]; then
        echo "Installing local Rustup..."
        mkdir -p "$RUSTUP_DIR"
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --default-toolchain stable
    fi
    # MIPS requires nightly for build-std (Tier 3)
    rustup toolchain install nightly
    rustup component add rust-src --toolchain nightly
}

setup_arch() {
    local arch_id=$1
    for entry in "${ARCH_MAP[@]}"; do
        IFS="|" read -r id target name url_file <<< "$entry"
        if [ "$arch_id" == "all" ] || [ "$arch_id" == "$id" ]; then
            echo "--- Setting up $id ($target) ---"
            
            # Download Toolchain
            mkdir -p "$TOOLCHAIN_DIR"
            if [ ! -d "$TOOLCHAIN_DIR/${name}-cross" ]; then
                echo "Downloading toolchain..."
                curl -L "https://musl.cc/$url_file" -o "$TOOLCHAIN_DIR/$url_file"
                tar -xf "$TOOLCHAIN_DIR/$url_file" -C "$TOOLCHAIN_DIR"
                rm "$TOOLCHAIN_DIR/$url_file"
            fi
            
            # Add Target
            rustup target add "$target"
            
            # Configure Cargo
            mkdir -p .cargo
            LINKER="$TOOLCHAIN_DIR/${name}-cross/bin/${name}-gcc"
            if ! grep -q "\[target.$target\]" .cargo/config.toml 2>/dev/null; then
                cat >> .cargo/config.toml <<EOF

[target.$target]
linker = "$LINKER"
rustflags = ["-C", "strip=symbols"]
EOF
            fi
        fi
    done
}

build_arch() {
    local arch_id=$1
    export RUSTFLAGS="--cfg reqwest_unstable -C strip=symbols"
    
    for entry in "${ARCH_MAP[@]}"; do
        IFS="|" read -r id target name url_file <<< "$entry"
        if [ "$arch_id" == "all" ] || [ "$arch_id" == "$id" ]; then
            echo "--- Building for $id ($target) ---"
            
            TARGET_ENV="${target//-/_}"
            export CC_${TARGET_ENV}="$TOOLCHAIN_DIR/${name}-cross/bin/${name}-gcc"
            export CXX_${TARGET_ENV}="$TOOLCHAIN_DIR/${name}-cross/bin/${name}-g++"
            export AR_${TARGET_ENV}="$TOOLCHAIN_DIR/${name}-cross/bin/${name}-ar"
            
            if [ "$id" == "mips" ]; then
                # Tier 3 targets like MIPS musl need build-std
                cargo +nightly build -Z build-std=std,panic_abort --target "$target" --profile release-mips
            else
                cargo build --release --target "$target"
            fi
        fi
    done
}

if [ $# -lt 2 ]; then usage; fi

COMMAND=$1
ARCH=$2

case $COMMAND in
    setup)
        setup_rust
        setup_arch "$ARCH"
        ;;
    build)
        build_arch "$ARCH"
        ;;
    *)
        usage
        ;;
esac