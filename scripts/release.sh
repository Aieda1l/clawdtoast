#!/bin/bash
# Builds a universal macOS binary, signs it, optionally notarizes it, and zips it to dist/.
#
#   scripts/release.sh                       # universal build, ad-hoc signature (runs locally only)
#   CODESIGN_IDENTITY="Developer ID Application: Name (TEAMID)" scripts/release.sh
#   CODESIGN_IDENTITY=... NOTARY_PROFILE=clawdtoast scripts/release.sh   # also notarize
#
# NOTARY_PROFILE is a keychain profile created once with:
#   xcrun notarytool store-credentials clawdtoast --apple-id you@example.com --team-id TEAMID
# Gatekeeper accepts a downloaded binary only when it is signed with a Developer ID
# identity and notarized; ad-hoc signatures are for local testing.
set -euo pipefail
cd "$(dirname "$0")/.."

IDENTITY="${CODESIGN_IDENTITY:--}"
TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)

for t in "${TARGETS[@]}"; do
  rustup target list --installed | grep -qx "$t" || rustup target add "$t"
  cargo build --locked --release --target "$t"
done

mkdir -p dist
lipo -create -output dist/clawdtoast \
  target/aarch64-apple-darwin/release/clawdtoast target/x86_64-apple-darwin/release/clawdtoast

if [ "$IDENTITY" = "-" ]; then
  echo "note: ad-hoc signature; Gatekeeper will block this on other machines" >&2
  codesign --force --sign - dist/clawdtoast
else
  codesign --force --timestamp --options runtime --sign "$IDENTITY" dist/clawdtoast
fi
codesign --verify --verbose=2 dist/clawdtoast

(cd dist && rm -f clawdtoast-macos.zip && zip -q clawdtoast-macos.zip clawdtoast)

if [ -n "${NOTARY_PROFILE:-}" ]; then
  [ "$IDENTITY" != "-" ] || { echo "notarization needs a Developer ID identity" >&2; exit 1; }
  xcrun notarytool submit dist/clawdtoast-macos.zip --keychain-profile "$NOTARY_PROFILE" --wait
  # A bare Mach-O cannot be stapled; Gatekeeper checks the notarization ticket online.
fi

lipo -info dist/clawdtoast
ls -l dist/clawdtoast-macos.zip
