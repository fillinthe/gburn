#!/bin/sh
# GBurn Link for macOS: universal (arm64 + x86_64) gburn-link, ad-hoc signed (unsigned arm64 binaries get SIGKILLed),
# zipped with its README as packaging/out/GBurn-Link-<version>-macOS.zip and copied to /Applications/GBurn Link/.
set -e
cd "$(dirname "$0")/../.."
export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH
V=$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)   # [workspace.package]
for t in aarch64-apple-darwin x86_64-apple-darwin; do
  RUSTFLAGS="--remap-path-prefix=$HOME=~ --remap-path-prefix=$(cd ../.. && pwd)=src" cargo build -q -p gburn-link --profile dist --target $t
done
OUT=packaging/out/link
mkdir -p "$OUT"
lipo -create -output "$OUT/gburn-link" target/aarch64-apple-darwin/dist/gburn-link target/x86_64-apple-darwin/dist/gburn-link
codesign --force -s - "$OUT/gburn-link"
cp packaging/macos/link-README.txt "$OUT/README.txt"
lipo -archs "$OUT/gburn-link"
rm -f "packaging/out/GBurn-Link-$V-macOS.zip"
(cd "$OUT" && zip -q "../GBurn-Link-$V-macOS.zip" gburn-link README.txt)
mkdir -p "/Applications/GBurn Link"
cp "$OUT/gburn-link" "$OUT/README.txt" "/Applications/GBurn Link/"
echo "packaging/out/GBurn-Link-$V-macOS.zip"
