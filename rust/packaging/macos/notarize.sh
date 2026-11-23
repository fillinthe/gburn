#!/bin/sh
# Send the signed dmg to Apple's notary service and staple the ticket (so it opens on any Mac without a warning).
# Needs an App Store Connect API key: GBURN_ASC_KEY (.p8 path), GBURN_ASC_KEY_ID, GBURN_ASC_ISSUER.
set -eu
OUT="$(cd "$(dirname "$0")/.." && pwd)/out"
DMG="$(ls -t "$OUT"/GBurn-*-macOS.dmg | head -1)"
xcrun notarytool submit "$DMG" --key "$GBURN_ASC_KEY" --key-id "$GBURN_ASC_KEY_ID" --issuer "$GBURN_ASC_ISSUER" --wait
xcrun stapler staple "$DMG"
xcrun stapler validate "$DMG"
