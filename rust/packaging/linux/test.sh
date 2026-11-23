#!/bin/sh
# Runs the packaged Linux build headless in Docker (Debian 12, Xvfb + Mesa llvmpipe) and saves out/shot-linux-<arch>.png:
# the window opened and OpenGL drew the sample model. Arch: aarch64 (native, default) or x86_64 (qemu, slow).
set -eu
PKG="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$PKG/out"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$(dirname "$PKG")/Cargo.toml" | head -1)"
export PATH="/opt/homebrew/bin:$PATH"
A="${1:-aarch64}"
[ "$A" = x86_64 ] && P=linux/amd64 || P=linux/arm64
[ -d "$OUT/GBurn-$VERSION-Linux-$A" ] || { echo "missing $OUT/GBurn-$VERSION-Linux-$A (sh build.sh)"; exit 1; }
rm -f "$OUT/shot-linux-$A.png"
docker run --rm --platform $P -v "$OUT:/out" debian:bookworm-slim sh -ec "
  apt-get update -qq && apt-get install -y -qq --no-install-recommends xvfb xauth libgl1 libegl1 libgl1-mesa-dri \
    libxkbcommon-x11-0 libxcursor1 libxi6 libxrandr2 >/dev/null
  cd /out/GBurn-$VERSION-Linux-$A
  GBURN_SHOT=/out/shot-linux-$A.png GBURN_SHOT_FRAME=12 LIBGL_ALWAYS_SOFTWARE=1 xvfb-run -s '-screen 0 1400x900x24' ./gburn"
test -s "$OUT/shot-linux-$A.png" && echo "PASS Linux $A" || { echo "FAIL Linux $A"; exit 1; }
