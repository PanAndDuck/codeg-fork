#!/usr/bin/env bash
unset CODEG_DATA_DIR CODEG_HOME CODEG_STATIC_DIR CODEG_ACP_TMP_ROOT CODEG_TOKEN CODEG_PORT CODEG_HOST
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$ROOT"
STAGE=false
BUNDLES=app
for arg in "$@"; do
  case "$arg" in
    --stage) STAGE=true ;;
    --bundles=app|--bundles=app,dmg) BUNDLES="${arg#--bundles=}" ;;
    --help|-h)
      echo "Usage: scripts/build-selfhost.sh [--stage] [--bundles=app|app,dmg]"
      echo "Stages only to ~/Applications/codeg-staged/codeg.app; never launches or installs."
      exit 0 ;;
    *) echo "Unknown option or destination: $arg" >&2; exit 2 ;;
  esac
done
[[ "$(uname -s)" == Darwin ]] || { echo "macOS is required" >&2; exit 1; }

DEST="${HOME:?}/Applications/codeg-staged/codeg.app"
check_stage_destination() {
  for dir in "$HOME/Applications" "$HOME/Applications/codeg-staged"; do
    [[ ! -L "$dir" ]] || { echo "Refusing redirected stage directory: $dir" >&2; exit 1; }
    if [[ -e "$dir" && "$(cd "$dir" && pwd -P)" != "$dir" ]]; then
      echo "Refusing redirected stage directory: $dir" >&2; exit 1
    fi
  done
  [[ ! -e "$DEST" && ! -L "$DEST" ]] || {
    echo "Refusing to replace an existing staged app: $DEST" >&2; exit 1;
  }
}
if $STAGE; then check_stage_destination; fi

# A selfhost build has the official identity, with updates explicitly disabled.
unset CODEG_FLAVOR NEXT_PUBLIC_CODEG_FLAVOR CODEG_SKIP_SIDECAR
unset CODEG_COMPUTER_PEER_REQUIREMENT CODEG_COMPUTER_HELPER_REQUIREMENT CODEG_COMPUTER_TEAM_ID
unset APPLE_ID APPLE_PASSWORD APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH APPLE_KEYCHAIN
unset TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD TAURI_CONFIG
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
export CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR="$ROOT/src-tauri/target"
export CODEG_UPDATER=off NEXT_PUBLIC_CODEG_UPDATER=off
CODEG_BUILD_COMMIT="$(git rev-parse --short HEAD)"
export CODEG_BUILD_COMMIT
# Certificate-backed builds pin both directions to the same certificate/team.
: "${CODEG_SELFHOST_SIGNING_IDENTITY:?Set CODEG_SELFHOST_SIGNING_IDENTITY to a certificate SHA-1, or - for ad-hoc}"
export APPLE_SIGNING_IDENTITY="$CODEG_SELFHOST_SIGNING_IDENTITY"
if [[ "$APPLE_SIGNING_IDENTITY" == - ]]; then
  export CODEG_COMPUTER_SIGNING=disabled
else
  : "${CODEG_SELFHOST_TEAM_ID:?Set CODEG_SELFHOST_TEAM_ID for certificate-backed builds}"
  [[ "$APPLE_SIGNING_IDENTITY" =~ ^[[:xdigit:]]{40}$ ]] || {
    echo "Signing identity must be an exact certificate SHA-1 (40 hex digits) or -" >&2; exit 2;
  }
  [[ "$CODEG_SELFHOST_TEAM_ID" =~ ^[A-Z0-9]{10}$ ]] || {
    echo "Team ID must be 10 uppercase letters/digits" >&2; exit 2;
  }
  export CODEG_COMPUTER_SIGNING="${CODEG_SELFHOST_SIGNING_KIND:-development}"
  case "$CODEG_COMPUTER_SIGNING" in
    development|developer-id) ;;
    *) echo "CODEG_SELFHOST_SIGNING_KIND must be development or developer-id" >&2; exit 2 ;;
  esac
  export CODEG_COMPUTER_TEAM_ID="$CODEG_SELFHOST_TEAM_ID"
  ANCHOR="anchor apple generic and certificate leaf = H\"$APPLE_SIGNING_IDENTITY\" and certificate leaf[subject.OU] = \"$CODEG_COMPUTER_TEAM_ID\""
  export CODEG_COMPUTER_PEER_REQUIREMENT="identifier \"app.codeg\" and $ANCHOR"
  export CODEG_COMPUTER_HELPER_REQUIREMENT="identifier \"app.codeg.computer-helper\" and $ANCHOR"
fi

BUILD_TMP="$(mktemp -d "${TMPDIR:-/tmp}/codeg-selfhost.XXXXXX")"
trap 'rm -rf "$BUILD_TMP"' EXIT
export CODEG_HOME="$BUILD_TMP/home" CODEG_DATA_DIR="$BUILD_TMP/data"
export CODEG_ACP_TMP_ROOT="$BUILD_TMP/acp"
mkdir -p "$CODEG_HOME" "$CODEG_DATA_DIR" "$CODEG_ACP_TMP_ROOT"

# Tauri merges this plist before signing; keep build-specific files temporary.
python3 - "$BUILD_TMP" "$CODEG_BUILD_COMMIT" <<'PY'
import json
from pathlib import Path
import plistlib
import sys

directory = Path(sys.argv[1])
plist = directory / 'Info.plist'
plist.write_bytes(plistlib.dumps({'CodegBuildCommit': sys.argv[2]}))
(directory / 'build-commit.conf.json').write_text(
    json.dumps({'bundle': {'macOS': {'infoPlist': str(plist)}}}), encoding='utf-8')
PY

PNPM_PACKAGE="$(node -p "require('./package.json').packageManager")"
if [[ "$(pnpm --version 2>/dev/null || true)" != "${PNPM_PACKAGE#pnpm@}" ]]; then
  pnpm() { npm exec --yes --package="$PNPM_PACKAGE" -- pnpm "$@"; }
fi
export CI=true
pnpm install --frozen-lockfile --prefer-offline

# Match release.yml: prepare and sign the nested app first, then let Tauri
# sign the main executable and MCP sidecar. No entitlements or notarization.
pnpm tauri:prepare-sidecars
HELPER="$ROOT/src-tauri/binaries/codeg-computer-helper.app"
if [[ "$APPLE_SIGNING_IDENTITY" == - ]]; then
  codesign --force --options runtime --sign - "$HELPER"
else
  codesign --force --options runtime --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$HELPER"
  codesign --verify --strict -R="$CODEG_COMPUTER_HELPER_REQUIREMENT" "$HELPER"
fi
export CODEG_SKIP_SIDECAR=1
pnpm tauri build --config src-tauri/tauri.selfhost.conf.json \
  --config "$BUILD_TMP/build-commit.conf.json" --bundles "$BUNDLES"

APP="$ROOT/src-tauri/target/release/bundle/macos/codeg.app"
PLIST="$APP/Contents/Info.plist"
IDENTIFIER="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$PLIST")"
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$PLIST")"
NAME="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleName' "$PLIST")"
SCHEME="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleURLTypes:0:CFBundleURLSchemes:0' "$PLIST")"
BUILD_COMMIT="$(/usr/libexec/PlistBuddy -c 'Print :CodegBuildCommit' "$PLIST")"
[[ "$BUILD_COMMIT" == "$CODEG_BUILD_COMMIT" ]] || {
  echo "Refusing missing or mismatched CodegBuildCommit" >&2; exit 1;
}
EXPECTED_VERSION="$(node -p "require('./src-tauri/tauri.conf.json').version")"
[[ "$IDENTIFIER" == app.codeg && "$NAME" == codeg && "$VERSION" == "$EXPECTED_VERSION" && "$SCHEME" == codeg ]] || {
  echo "Refusing unexpected app identity/version" >&2; exit 1;
}
codesign --verify --deep --strict "$APP"
if [[ "$APPLE_SIGNING_IDENTITY" != - ]]; then
  codesign --verify --strict -R="$CODEG_COMPUTER_PEER_REQUIREMENT" "$APP"
  codesign --verify --strict -R="$CODEG_COMPUTER_HELPER_REQUIREMENT" "$APP/Contents/Helpers/codeg-computer-helper.app"
fi
for code in "$APP/Contents/MacOS/codeg" "$APP/Contents/MacOS/codeg-mcp" \
  "$APP/Contents/Helpers/codeg-computer-helper.app" \
  "$APP/Contents/Helpers/codeg-computer-helper.app/Contents/MacOS/codeg-computer-helper"; do
  summary="$(codesign -dvvv "$code" 2>&1)"
  if [[ "$APPLE_SIGNING_IDENTITY" == - ]]; then
    grep -q '^Signature=adhoc$' <<< "$summary"
  else
    if [[ "$CODEG_COMPUTER_SIGNING" == development ]]; then
      grep -q '^Authority=Apple Development:' <<< "$summary"
    else
      grep -q '^Authority=Developer ID Application:' <<< "$summary"
    fi
    grep -q "^TeamIdentifier=$CODEG_COMPUTER_TEAM_ID$" <<< "$summary"
    grep -q 'flags=.*(runtime)' <<< "$summary"
    codesign --verify --strict -R="$ANCHOR" "$code"
  fi
  if codesign -d --entitlements - --xml "$code" 2>/dev/null | grep -q '<key>'; then
    echo "Refusing unexpected entitlements: $code" >&2; exit 1
  fi
done
if xattr -lr "$APP" | grep -q 'com.apple.quarantine'; then
  echo "Refusing quarantined build" >&2; exit 1
fi

if $STAGE; then
  check_stage_destination
  mkdir -p "$HOME/Applications/codeg-staged"
  check_stage_destination
  ditto "$APP" "$DEST"
  codesign --verify --deep --strict "$DEST"
  APP="$DEST"
fi
printf 'App: %s\nIdentifier: %s\nVersion: %s\nCommit: %s\nUpdater: off\n' "$APP" "$IDENTIFIER" "$VERSION" "$CODEG_BUILD_COMMIT"
codesign -dvvv "$APP" 2>&1 | grep -E '^(Identifier|Format|CodeDirectory|Signature|Authority|TeamIdentifier|Timestamp)'
echo "Not launched. Installation is a separate task."
