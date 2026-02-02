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
    "arm64|aarch64-unknown-linux-musl|aarch64-linux-musl|aarch64-linux-musl-cross.tgz"
)

usage() {
    echo "Usage: $0 [setup|build] [arch|all]"
    echo "Architectures: arm64"
    exit 1
}

setup_rust() {
    if [ ! -d "$RUSTUP_DIR" ]; then
        echo "Installing local Rustup..."
        mkdir -p "$RUSTUP_DIR"
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --default-toolchain stable
    fi
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
            
            cargo build --release --target "$target"
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