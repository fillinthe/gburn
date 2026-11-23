#!/bin/sh
# macOS: universal (arm64 + x86_64) GBurn.app + .dmg in rust/packaging/out/ (local only: no public downloads).
# Sign identity: GBURN_CODESIGN_IDENTITY (e.g. the local "GCarve Dev" cert: macOS then stops re-asking for permission
# after each build; its keychain must be unlocked, or codesign waits for a password prompt). Default ad-hoc ("-").
set -eu
PKG="$(cd "$(dirname "$0")/.." && pwd)"
RUST="$(dirname "$PKG")"
OUT="$PKG/out"
mkdir -p "$OUT"
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$RUST/Cargo.toml" | head -1)"
cd "$RUST"
rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
# the screenshot feature only acts when GBURN_SHOT is set: test.sh uses it to prove the window renders
# no home folder (user name) and no folder names above the projects in the public files
export RUSTFLAGS="--remap-path-prefix=$HOME=~ --remap-path-prefix=$(cd ../.. && pwd)=src"
MACOSX_DEPLOYMENT_TARGET=11.0 cargo build -p gburn-app --profile dist --features screenshot --target aarch64-apple-darwin
MACOSX_DEPLOYMENT_TARGET=10.13 cargo build -p gburn-app --profile dist --features screenshot --target x86_64-apple-darwin

APP="$OUT/GBurn.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/MacOS/gburn" target/aarch64-apple-darwin/dist/gburn target/x86_64-apple-darwin/dist/gburn
cp "$PKG/macos/GBurn.icns" "$APP/Contents/Resources/"
sed "s/VERSION/$VERSION/g" "$PKG/macos/Info.plist" > "$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist" >/dev/null
ID="${GBURN_CODESIGN_IDENTITY:--}"
if [ "$ID" = "-" ]; then
    codesign --force --deep --sign - "$APP"
else
    # a real identity (Developer ID Application): hardened runtime + timestamp, which Apple's notarization requires;
    # GBURN_KEYCHAIN names the keychain holding it. The camera entitlement is for Camera Overlay.
    codesign --force --deep --options runtime --timestamp --entitlements "$PKG/macos/GBurn.entitlements" ${GBURN_KEYCHAIN:+--keychain "$GBURN_KEYCHAIN"} --sign "$ID" "$APP"
fi

DMG="$OUT/GBurn-$VERSION-macOS.dmg"
rm -f "$DMG"
# dmgbuild (pipx install dmgbuild) lays out the window (background, icon spots) without Finder
command -v dmgbuild >/dev/null || PATH="$HOME/.local/bin:$PATH"
dmgbuild -s "$PKG/macos/dmg-settings.py" -D app="$APP" -D bg="$PKG/macos/dmg-background.tiff" GBurn "$DMG" >/dev/null
lipo -info "$APP/Contents/MacOS/gburn"
codesign -dv "$APP" 2>&1 | grep -E "Authority|Signature" | head -2
ls -l "$DMG"
