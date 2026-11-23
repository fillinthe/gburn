#!/bin/sh
# Windows, cross-compiled with mingw-w64, the same recipe as GCarve's rust/packaging/windows/build.sh:
#   sh build.sh x64        Windows 10/11 x64   (x86_64-pc-windows-gnu, stable; on the Mac: brew install mingw-w64)
#   sh build.sh win7-x64   Windows 7/8 x64     (x86_64-win7-windows-gnu, tier 3: nightly + build-std)
#   sh build.sh win7-x86   Windows 7/8 32-bit  (i686-win7-windows-gnu,  tier 3: nightly + build-std)
# The win7 builds run in GCarve's Docker build image with both repos mounted (gburn-app depends on ../gcarve by
# path): Debian's mingw-w64 links msvcrt.dll, which every Windows has. combase.dll imports are moved to ole32.dll
# by GCarve's windows/imports.py, and the build fails if any Windows 8+ DLL is still imported.
# -> rust/packaging/out/GBurn-<version>-Windows-<variant>.zip (gburn.exe, README.txt). Local only: no public downloads.
set -eu
PKG="$(cd "$(dirname "$0")/.." && pwd)"
RUST="$(dirname "$PKG")"
REPO="$(dirname "$RUST")"
PARENT="$(dirname "$REPO")"
GC="$PARENT/gcarve/rust/packaging"
OUT="$PKG/out"
HERE="$PKG/windows"
mkdir -p "$OUT"
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$RUST/Cargo.toml" | head -1)"
VARIANT="${1:-x64}"; EXTRA=""
case "$VARIANT" in
  x64)      T=x86_64-pc-windows-gnu;  CC=x86_64-w64-mingw32; TC=stable ;;
  win7-x64) T=x86_64-win7-windows-gnu; CC=x86_64-w64-mingw32; TC=nightly ;;
  win7-x86) T=i686-win7-windows-gnu;  CC=i686-w64-mingw32;   TC=nightly; EXTRA="-C panic=abort" ;;  # mingw i686 unwinding (SJLJ) is not what Rust expects
  *) echo "usage: $0 x64|win7-x64|win7-x86"; exit 2 ;;
esac
if [ $TC = nightly ] && [ ! -f /.dockerenv ]; then
  docker build -q -t gcarve-rust-build -f "$GC/docker/Dockerfile.build" "$GC/docker" >/dev/null
  exec docker run --rm -v "$PARENT:/src" -v gcarve-rust-registry:/opt/cargo/registry \
    -e CARGO_TARGET_DIR=/src/gburn/rust/target/docker -w /src/gburn gcarve-rust-build sh rust/packaging/windows/build.sh "$VARIANT"
fi
command -v $CC-gcc >/dev/null || { echo "needs mingw-w64: brew install mingw-w64"; exit 1; }
TARGET_DIR="${CARGO_TARGET_DIR:-$RUST/target}"
cd "$RUST"

# icon + version resource, linked straight into the exe
OBJ="$OUT/obj-$VARIANT"; mkdir -p "$OBJ"
V4="$(echo "$VERSION" | tr . ,),0"
sed -e "s#@ICON@#$HERE/gburn.ico#" -e "s/@V4@/$V4/g" -e "s/@VERSION@/$VERSION/g" "$HERE/gburn.rc.in" > "$OBJ/gburn.rc"
$CC-windres "$OBJ/gburn.rc" -O coff -o "$OBJ/gburn-res.o"

TUP="$(echo $T | tr a-z- A-Z_)"
export CARGO_TARGET_${TUP}_LINKER=$CC-gcc
# GUI subsystem (no console window); the resource object carries icon + version info
export CARGO_TARGET_${TUP}_RUSTFLAGS="-C link-arg=-Wl,--subsystem,windows -C link-arg=$OBJ/gburn-res.o --remap-path-prefix=$HOME=~ --remap-path-prefix=$PARENT=src $EXTRA" # no home folder (user name) or folder names in the files
export CC_$(echo $T | tr - _)=$CC-gcc
if [ $TC = nightly ]; then
  rustup component add rust-src --toolchain nightly >/dev/null
  cargo +nightly build -p gburn-app --profile dist --features screenshot --target $T -Zbuild-std=std,panic_abort
else
  rustup target add $T >/dev/null
  cargo build -p gburn-app --profile dist --features screenshot --target $T
  env "CARGO_TARGET_${TUP}_RUSTFLAGS=--remap-path-prefix=$HOME=~ --remap-path-prefix=$PARENT=src" cargo build -p gburn-link --profile dist --target $T
fi

NAME="GBurn-$VERSION-Windows-$VARIANT"
STAGE="$OUT/$NAME"; rm -rf "$STAGE" "$STAGE.zip"; mkdir -p "$STAGE"
cp "$TARGET_DIR/$T/dist/gburn.exe" "$STAGE/gburn.exe"
[ -f "$TARGET_DIR/$T/dist/gburn-link.exe" ] && cp "$TARGET_DIR/$T/dist/gburn-link.exe" "$STAGE/gburn-link.exe"  # the browser-to-Ruida helper (x64 only)
case $VARIANT in win7-*)  # combase.dll -> ole32.dll, then fail if any Windows 8+ DLL is still imported
  python3 "$GC/windows/imports.py" "$STAGE/gburn.exe" --fix-combase > "$OBJ/imports.txt" || { cat "$OBJ/imports.txt"; exit 1; }
  tail -1 "$OBJ/imports.txt" ;;
esac
# software drawing for PCs / VMs without OpenGL 2 (gburn.exe restarts on it when the driver's GL fails): Mesa's
# llvmpipe, in software/ so it's only loaded on purpose (MIT; pinned, checked, cached outside git; UCRT = Windows 10+ only)
if [ "$VARIANT" = x64 ]; then
  MV=26.2.6; MSUM=da0f71d9181b672c1918f3c342f11118c687f6059a46b16502845160733cc388
  MC="$HERE/.mesa-$MV"; MZ="$MC/mesa3d-$MV-release-mingw.7z"
  if [ ! -f "$MC/x64/libgallium_wgl.dll" ]; then
    mkdir -p "$MC"
    [ -f "$MZ" ] || curl -sSL -o "$MZ" "https://github.com/pal1000/mesa-dist-win/releases/download/$MV/mesa3d-$MV-release-mingw.7z"
    echo "$MSUM  $MZ" | shasum -a 256 -c - >/dev/null || { echo "Mesa download checksum mismatch"; exit 1; }
    bsdtar -xf "$MZ" -C "$MC" x64/opengl32.dll x64/libgallium_wgl.dll
  fi
  mkdir -p "$STAGE/software"  # not beside gburn.exe: there it would replace the PC's own (GPU) OpenGL
  cp "$MC/x64/opengl32.dll" "$MC/x64/libgallium_wgl.dll" "$STAGE/software/"
fi
sed 's/$/\r/' "$HERE/README.txt" > "$STAGE/README.txt"
(cd "$OUT" && zip -qr "$NAME.zip" "$NAME")
ls -l "$STAGE/gburn.exe" "$STAGE.zip"
