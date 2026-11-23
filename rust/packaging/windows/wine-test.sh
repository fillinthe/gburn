#!/bin/sh
# Runs the packaged Windows build under Wine in Docker (no Windows PC needed), the way GCarve's
# rust/packaging/windows/wine-test.sh does: same image (Dockerfile.wine over there), same Colima profile `gcarve-wine`.
#   sh wine-test.sh [x64] [win7-x64] [win7-x86]     (default: x64, the one GBurn ships; build it first with build.sh)
# gburn.exe starts with GBURN_SHOT set and must write out/shot-wine-<variant>.png: the window opened and OpenGL drew.
set -eu
PKG="$(cd "$(dirname "$0")/.." && pwd)"
RUST="$(dirname "$PKG")"
OUT="$PKG/out"
GC="$(cd "$RUST/../../gcarve/rust/packaging" && pwd)"
export PATH="/opt/homebrew/bin:$PATH"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$RUST/Cargo.toml" | head -1)"
VARIANTS="${*:-x64}"
colima status -p gcarve-wine >/dev/null 2>&1 ||
  colima start gcarve-wine --vm-type vz --vz-rosetta --arch aarch64 --cpu 8 --memory 8 --disk 30 --mount-type virtiofs
export DOCKER_HOST="unix://$HOME/.colima/gcarve-wine/docker.sock"   # never touch the default profile
docker build -q --platform linux/amd64 -t gcarve-wine-test -f "$GC/windows/Dockerfile.wine" "$GC/windows" >/dev/null
FAIL=0
for V in $VARIANTS; do
  NAME="GBurn-$VERSION-Windows-$V"
  [ -f "$OUT/$NAME.zip" ] || { echo "missing $OUT/$NAME.zip (sh build.sh $V)"; FAIL=1; continue; }
  rm -f "$OUT/shot-wine-$V.png"
  docker run --rm --platform linux/amd64 -v "$OUT:/out" -e V="$V" -e NAME="$NAME" gcarve-wine-test sh -c '
    Xvfb :9 -screen 0 1400x900x24 >/dev/null 2>&1 & sleep 2; export DISPLAY=:9
    rm -rf /tmp/w && mkdir /tmp/w && cd /tmp/w && unzip -q "/out/$NAME.zip" && cd "$NAME"
    export WINEDEBUG=err+all,fixme-all
    GBURN_SHOT="Z:\\out\\shot-wine-$V.png" GBURN_SHOT_FRAME=12 timeout 900 wine gburn.exe
    echo "exit $?"
    wineserver -k 2>/dev/null; true'
  if [ -s "$OUT/shot-wine-$V.png" ]; then echo "PASS shot-wine-$V.png"; else echo "FAIL $V: no screenshot from gburn.exe under Wine"; FAIL=1; fi
done
exit $FAIL
