#!/usr/bin/env bash
# Signs, notarizes and staples an UNSIGNED macOS build of Brainitive Desktop (codex review 11):
# the build (`npx tauri build`, npm, cargo and every crate's build script) never sees a signing
# secret; only this script, which runs Apple's own tools, does. Used by CI (.github/workflows/
# build.yml, step "macOS signing + notarization") and on a signing Mac (docs/APPLE-SIGNING.md § 5).
#
#   scripts/macos-sign.sh <path/to/Brainitive.app> <path/to/Brainitive_X.Y.Z_universal.dmg>
#
# What it does, in order:
#   1. the identity: APPLE_CERTIFICATE (base64 .p12) + APPLE_CERTIFICATE_PASSWORD go into a
#      throwaway keychain (CI); without them the identity must already be in the user's keychain
#   2. codesign the .app: hardened runtime, src-tauri/entitlements.plist, secure timestamp
#   3. a new .dmg from the signed .app (Tauri's own .dmg holds the unsigned one), signed too
#   4. notarytool submit the .dmg (--wait; Apple checks the app inside), then staple the .dmg and
#      the .app (the updater archive is made from the stapled .app afterwards)
#
# Environment:
#   APPLE_SIGNING_IDENTITY      required, "Developer ID Application: Name (TEAMID)"
#   APPLE_CERTIFICATE           optional, base64 of the .p12 (CI)
#   APPLE_CERTIFICATE_PASSWORD  with APPLE_CERTIFICATE
#   notarization, one of (none: signed but not notarized, with a warning):
#     A: APPLE_ID, APPLE_PASSWORD (app-specific), APPLE_TEAM_ID
#     B: APPLE_API_KEY (Key ID), APPLE_API_ISSUER, and APPLE_API_KEY_P8 (the .p8 text, CI) or
#        APPLE_API_KEY_PATH (the .p8 file)
# Nothing secret is printed; the temporary keychain and key file are deleted on exit.
# Runs with macOS' bash 3.2 (empty arrays are expanded with ${a[@]+"${a[@]}"} under set -u).
set -euo pipefail

app="${1:?usage: macos-sign.sh <Brainitive.app> <Brainitive.dmg>}"
dmg="${2:?usage: macos-sign.sh <Brainitive.app> <Brainitive.dmg>}"
[ -d "$app" ] || { echo "error: no app at $app" >&2; exit 1; }
[ -n "${APPLE_SIGNING_IDENTITY:-}" ] || { echo "error: APPLE_SIGNING_IDENTITY is not set" >&2; exit 1; }
repo="$(cd "$(dirname "$0")/.." && pwd)"
entitlements="$repo/src-tauri/entitlements.plist"
[ -f "$entitlements" ] || { echo "error: no $entitlements" >&2; exit 1; }

work="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/brainitive-sign.XXXXXX")"
chmod 700 "$work"
keychain=""
cleanup() {
  if [ -n "$keychain" ]; then
    security delete-keychain "$keychain" 2>/dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT

# 1. the signing identity
keychain_args=()
if [ -n "${APPLE_CERTIFICATE:-}" ]; then
  keychain="$work/signing.keychain-db"
  keychain_password="$(openssl rand -hex 24)"
  (umask 077 && printf '%s' "$APPLE_CERTIFICATE" | base64 --decode > "$work/certificate.p12")
  security create-keychain -p "$keychain_password" "$keychain"
  security set-keychain-settings -lut 3600 "$keychain"
  security unlock-keychain -p "$keychain_password" "$keychain"
  security import "$work/certificate.p12" -k "$keychain" -P "${APPLE_CERTIFICATE_PASSWORD:-}" \
    -T /usr/bin/codesign > /dev/null
  rm -f "$work/certificate.p12"
  security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$keychain_password" "$keychain" > /dev/null
  # codesign looks the certificate chain up in the search list
  existing=()
  while IFS= read -r line; do
    line="${line#"${line%%[![:space:]]*}"}"
    existing+=("${line//\"/}")
  done < <(security list-keychains -d user)
  security list-keychains -d user -s "$keychain" "${existing[@]}"
  keychain_args=(--keychain "$keychain")
fi
echo "signing with the Developer ID identity"

# 2. the app
codesign --force --deep --options runtime --timestamp \
  --entitlements "$entitlements" ${keychain_args[@]+"${keychain_args[@]}"} \
  --sign "$APPLE_SIGNING_IDENTITY" "$app"
codesign --verify --deep --strict --verbose=2 "$app"

# 3. a fresh dmg with the signed app (and the usual Applications link), signed
stage="$work/dmg"
mkdir -p "$stage"
ditto "$app" "$stage/$(basename "$app")"
ln -s /Applications "$stage/Applications"
rm -f "$dmg"
mkdir -p "$(dirname "$dmg")"
hdiutil create -volname "Brainitive" -srcfolder "$stage" -fs HFS+ -format UDZO -ov "$dmg" > /dev/null
codesign --force --timestamp ${keychain_args[@]+"${keychain_args[@]}"} --sign "$APPLE_SIGNING_IDENTITY" "$dmg"
codesign --verify --verbose=2 "$dmg"

# 4. notarization
notary=()
if [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_PASSWORD:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ]; then
  notary=(--apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID")
elif [ -n "${APPLE_API_KEY:-}" ] && [ -n "${APPLE_API_ISSUER:-}" ]; then
  key_path="${APPLE_API_KEY_PATH:-}"
  if [ -n "${APPLE_API_KEY_P8:-}" ]; then
    key_path="$work/AuthKey_${APPLE_API_KEY}.p8"
    (umask 077 && printf '%s' "$APPLE_API_KEY_P8" > "$key_path")
  fi
  [ -f "$key_path" ] || { echo "error: no .p8 (APPLE_API_KEY_P8 or APPLE_API_KEY_PATH)" >&2; exit 1; }
  notary=(--key "$key_path" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER")
fi
if [ -z "${notary[*]:-}" ]; then
  echo "warning: no notarization credentials: signed but NOT notarized (Gatekeeper will warn)" >&2
  exit 0
fi
echo "notarizing (this waits for Apple)"
result="$(xcrun notarytool submit "$dmg" "${notary[@]}" --wait --timeout 1h --output-format json)"
status="$(printf '%s' "$result" | /usr/bin/python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("status",""))')"
id="$(printf '%s' "$result" | /usr/bin/python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("id",""))')"
if [ "$status" != "Accepted" ]; then
  echo "error: notarization status '$status' (submission $id); see: xcrun notarytool log $id <credentials>" >&2
  exit 1
fi
xcrun stapler staple "$dmg"
xcrun stapler staple "$app"
xcrun stapler validate "$dmg"
xcrun stapler validate "$app"
spctl --assess --type execute --verbose=2 "$app"
echo "signed, notarized and stapled: $(basename "$app"), $(basename "$dmg")"
