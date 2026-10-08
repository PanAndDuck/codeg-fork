#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
export CODEG_FLAVOR=fork NEXT_PUBLIC_CODEG_FLAVOR=fork
export CARGO_TARGET_DIR="$ROOT/src-tauri/target"
export CODEG_HOST=127.0.0.1
export CODEG_PORT="${CODEG_PORT:-3091}"
# Keep headless checks within the explicitly selected test listener.
export CODEG_BRIDGE_PORTS=off
case "$CODEG_PORT" in
  309[0-9]|184[0-9][0-9]) ;;
  *) echo "Fork development port must be 3090-3099 or 18400-18499" >&2; exit 2 ;;
esac

if [[ $# -gt 0 ]]; then
  echo "Usage: scripts/fork-server-dev.sh (uses a fresh temporary home/data directory)" >&2
  exit 2
fi
STATE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/codeg-fork-dev.XXXXXX")"
export CODEG_HOME="$STATE_DIR/home" CODEG_DATA_DIR="$STATE_DIR/data"
export CODEG_ACP_TMP_ROOT="$STATE_DIR/tmp"
export CODEG_TOKEN="$(openssl rand -hex 24)"
export CODEG_STATIC_DIR="$ROOT/out"
mkdir -p "$CODEG_HOME" "$CODEG_DATA_DIR" "$CODEG_ACP_TMP_ROOT"
SERVER="$CARGO_TARGET_DIR/debug/codeg-server"
SERVER_PID=
cleanup() {
  if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
    COMMAND="$(ps -p "$SERVER_PID" -o command=)"
    if [[ "$COMMAND" == "$SERVER" || "$COMMAND" == "$SERVER "* ]]; then
      kill "$SERVER_PID"
      wait "$SERVER_PID" 2>/dev/null || true
    fi
  fi
  echo "Fork state retained for inspection: $STATE_DIR"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

cd "$ROOT/src-tauri"
cargo build --no-default-features --features server-bin --bin codeg-server
echo "URL: http://$CODEG_HOST:$CODEG_PORT"
echo "Token: $CODEG_TOKEN"
echo "Home: $CODEG_HOME"
echo "Data: $CODEG_DATA_DIR"
"$SERVER" &
SERVER_PID=$!
echo "PID: $SERVER_PID"
wait "$SERVER_PID"
