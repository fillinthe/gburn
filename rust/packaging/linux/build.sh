#!/bin/sh
# Linux x86_64 + arm64, built in Docker with GCarve's build image (../../gcarve/rust/packaging/docker/Dockerfile.build,
# Debian 11 = glibc 2.31 floor). Both repos are mounted (gburn-app depends on ../gcarve/rust/gcarve-ui by path):
# the folder holding gcarve/ and gburn/ is /src in the container.
# -> rust/packaging/out/GBurn-<version>-Linux-<arch>.tar.gz (gburn, gburn.desktop, gburn.png, install.sh)
set -eu
PKG="$(cd "$(dirname "$0")/.." && pwd)"
RUST="$(dirname "$PKG")"
REPO="$(dirname "$RUST")"
PARENT="$(dirname "$REPO")"
GC="$PARENT/gcarve/rust/packaging"
OUT="$PKG/out"
mkdir -p "$OUT"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$RUST/Cargo.toml" | head -1)"
if [ ! -f /.dockerenv ]; then
  export PATH="/opt/homebrew/bin:$PATH"
  docker build -q -t gcarve-rust-build -f "$GC/docker/Dockerfile.build" "$GC/docker" >/dev/null
  exec docker run --rm -v "$PARENT:/src" -v gcarve-rust-registry:/opt/cargo/registry \
    -e CARGO_TARGET_DIR=/src/gburn/rust/target/docker -w /src/gburn gcarve-rust-build sh rust/packaging/linux/build.sh
fi
cd "$RUST"
rustup target add aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu >/dev/null 2>&1 || true
for T in aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu; do
  cargo build -p gburn-app --profile dist --features screenshot --target "$T"
  cargo build -p gburn-link --profile dist --target "$T"
  NAME="GBurn-$VERSION-Linux-${T%%-*}"
  STAGE="$OUT/$NAME"; rm -rf "$STAGE" "$STAGE.tar.gz"; mkdir -p "$STAGE"
  cp "${CARGO_TARGET_DIR:-$RUST/target}/$T/dist/gburn" "${CARGO_TARGET_DIR:-$RUST/target}/$T/dist/gburn-link" "$STAGE/"
  cp "$PKG/linux/gburn.desktop" "$PKG/linux/install.sh" "$STAGE/"
  cp "$REPO/rust/gburn-app/icon-512.png" "$STAGE/gburn.png"  # rounded, clear corners (the web one is a full square)
  tar -C "$OUT" -czf "$STAGE.tar.gz" "$NAME"
  echo "$NAME: glibc symbols up to $(objdump -T "$STAGE/gburn" 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1)"
  ls -l "$STAGE/gburn" "$STAGE.tar.gz"
done
