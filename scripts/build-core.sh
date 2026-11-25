#!/usr/bin/env bash
# Biên dịch lõi Rust thành thư viện tĩnh cho iPhone (arm64). Chạy trên macOS có Xcode.
set -euo pipefail
cd "$(dirname "$0")/../core"
export IPHONEOS_DEPLOYMENT_TARGET=17.0
rustup target add aarch64-apple-ios
cargo build --release --target aarch64-apple-ios
ls -lh target/aarch64-apple-ios/release/liblocsim_core.a
