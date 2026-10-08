#!/usr/bin/env bash
unset CODEG_DATA_DIR CODEG_HOME CODEG_STATIC_DIR CODEG_ACP_TMP_ROOT CODEG_TOKEN CODEG_PORT CODEG_HOST
if env | grep '^CODEG' >/dev/null; then
  echo "Clear remaining CODEG environment variables before building the fork" >&2
  exit 1
fi
set -euo pipefail

# Builds launched by the official app must never inherit its runtime state.
CODEG_DATA_DIR=$(mktemp -d)
CODEG_HOME=$(mktemp -d)
export CODEG_DATA_DIR CODEG_HOME

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$ROOT"
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
export CODEG_FLAVOR=fork NEXT_PUBLIC_CODEG_FLAVOR=fork
export CARGO_TARGET_DIR="$ROOT/src-tauri/target"
# The release CI's official trust anchors must never enter an ad-hoc fork.
unset CODEG_SKIP_SIDECAR CODEG_COMPUTER_PEER_REQUIREMENT
unset CODEG_COMPUTER_HELPER_REQUIREMENT CODEG_COMPUTER_TEAM_ID
export APPLE_SIGNING_IDENTITY=-
# Keep the program's pnpm 10 requirement local; do not alter packageManager or
# the user's Corepack/pnpm configuration (the upstream package pins pnpm 11).
export npm_config_manage_package_manager_versions=false
export npm_config_package_manager_strict=false
pnpm() { npm exec --yes --package=pnpm@10.34.6 -- pnpm "$@"; }

BUNDLES=app
INSTALL=false
for arg in "$@"; do
  case "$arg" in
    --dmg) BUNDLES=app,dmg ;;
    --install) INSTALL=true ;;
    --help|-h)
      echo "Usage: scripts/build-fork.sh [--dmg] [--install]"
      exit 0 ;;
    *) echo "Unknown option: $arg" >&2; exit 2 ;;
  esac
done

pnpm install --frozen-lockfile --prefer-offline
pnpm tauri build --config src-tauri/tauri.fork.conf.json --bundles "$BUNDLES"

APP="$ROOT/src-tauri/target/release/bundle/macos/Codeg-Fork.app"
[[ -d "$APP" ]] || { echo "Missing fork bundle: $APP" >&2; exit 1; }
IDENTIFIER="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP/Contents/Info.plist")"
[[ "$IDENTIFIER" == app.codeg.fork ]] || { echo "Refusing non-fork bundle" >&2; exit 1; }
echo "Built: $APP"

if $INSTALL; then
  DEST="${HOME:?}/Applications/Codeg-Fork.app"
  [[ "$(basename "$DEST")" == Codeg-Fork.app && ! -L "$DEST" ]] || {
    echo "Refusing unsafe install destination: $DEST" >&2; exit 1;
  }
  mkdir -p "${HOME}/Applications"
  # Reject a redirected ~/Applications: installation is confined to this home.
  [[ "$(cd "${HOME}/Applications" && pwd -P)" == "${HOME}/Applications" ]] || {
    echo "Refusing redirected Applications directory" >&2; exit 1;
  }
  if [[ -e "$DEST" ]]; then
    echo "Refusing to replace an existing app; move $DEST aside explicitly first" >&2
    exit 1
  fi
  ditto "$APP" "$DEST"
  echo "Installed: $DEST"
fi
