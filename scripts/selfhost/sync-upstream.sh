#!/usr/bin/env bash
unset CODEG_DATA_DIR CODEG_HOME CODEG_STATIC_DIR CODEG_ACP_TMP_ROOT CODEG_TOKEN CODEG_PORT CODEG_HOST
set -euo pipefail

[[ $# == 1 && "$1" != -* ]] || { echo 'Usage: sync-upstream.sh <upstream-tag>' >&2; exit 2; }
TAG=$1
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
cd "$ROOT"
git check-ref-format "refs/heads/sync/$TAG" >/dev/null
git check-ref-format "refs/tags/$TAG" >/dev/null
[[ -z "$(git status --porcelain)" ]] || { echo 'Commit/stash changes before syncing' >&2; exit 1; }
git rev-parse --verify refs/heads/fork/main >/dev/null
if git show-ref --verify --quiet "refs/heads/sync/$TAG"; then
  echo "Branch sync/$TAG already exists; inspect it before retrying" >&2
  exit 1
fi
# No inherited relay or agent settings may escape into build/test processes.
if env | grep '^CODEG' | grep -qv '^CODEG_SWAP_BACKUP_DIR='; then echo 'Clear remaining CODEG environment variables first' >&2; exit 1; fi
umask 077
BACKUPS="${CODEG_SWAP_BACKUP_DIR:-${HOME:?}/codeg-backups}"
unset CODEG_SWAP_BACKUP_DIR
mkdir -p "$BACKUPS/logs"
LOG="$BACKUPS/logs/sync-$(date '+%Y%m%dT%H%M%S')-$$.log"
exec > >(tee -a "$LOG") 2>&1
run() { printf '+ '; printf '%q ' "$@"; printf '\n'; "$@"; }
TEMP_STATE=$(mktemp -d "${TMPDIR:-/tmp}/codeg-sync.XXXXXX")
E2E_NAME="selfhost-sync-$$"
E2E_STARTED=false
cleanup() {
  local rc=$?
  trap - EXIT
  if $E2E_STARTED; then
    env -u CODEG_DATA_DIR -u CODEG_HOME -u CODEG_ACP_TMP_ROOT -u CODEG_PORT -u CODEG_HOST -u CODEG_BRIDGE_PORTS \
      "$ROOT/e2e/env-down.sh" --name "$E2E_NAME" || rc=1
  fi
  rm -rf "$TEMP_STATE"
  echo "Sync exit=$rc; log=$LOG"
  exit "$rc"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
run git fetch upstream --tags
git rev-parse --verify "refs/tags/$TAG^{commit}" >/dev/null
run git switch -c "sync/$TAG" fork/main
printf 'Merge upstream %s\n\nCo-Authored-By: Codex CLI <noreply@openai.com>\n' "$TAG" > "$TEMP_STATE/merge-message"
if ! run git -c core.editor=true merge --no-edit --file "$TEMP_STATE/merge-message" "refs/tags/$TAG"; then
  echo 'Merge stopped. Resolve these conflicts and commit manually; no build was started:'
  git diff --name-only --diff-filter=U
  exit 1
fi
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
export CARGO_TARGET_DIR="$ROOT/src-tauri/target"
export CODEG_DATA_DIR="$TEMP_STATE/data" CODEG_HOME="$TEMP_STATE/home"
export CODEG_ACP_TMP_ROOT="$TEMP_STATE/acp" CODEG_HOST=127.0.0.1 CODEG_PORT=3098 CODEG_BRIDGE_PORTS=off
mkdir -p "$CODEG_DATA_DIR" "$CODEG_HOME" "$CODEG_ACP_TMP_ROOT"
run pnpm install --frozen-lockfile --prefer-offline
run pnpm lint .
run pnpm test
run pnpm build
(
  cd "$ROOT/src-tauri"
  run cargo check
  run cargo test --features test-utils
  run cargo clippy --all-targets --features test-utils -- -D warnings
  run cargo check --no-default-features --features server-bin --bin codeg-server
  run cargo test --no-default-features --features server-bin --bin codeg-server --lib
  run cargo clippy --no-default-features --features server-bin --bin codeg-server --lib -- -D warnings
  run cargo check --no-default-features --features mcp-bin --bin codeg-mcp
  run cargo test --no-default-features --features mcp-bin --bin codeg-mcp
  run cargo clippy --no-default-features --features mcp-bin --bin codeg-mcp -- -D warnings
)
for crate in codeg-tunnel codeg-relay; do
  run cargo test --manifest-path "$ROOT/crates/$crate/Cargo.toml"
done
unset CODEG_DATA_DIR CODEG_HOME CODEG_ACP_TMP_ROOT CODEG_PORT CODEG_HOST CODEG_BRIDGE_PORTS
if [[ -f e2e/smoke.py && -x e2e/env-up.sh ]]; then
  E2E_STARTED=true
  run e2e/env-up.sh --name "$E2E_NAME" --relay-port 18473 --server-port 3098 --proxy-port 18474 --control-port 18475 --proxy-mode terminate
  run python3 e2e/smoke.py --name "$E2E_NAME"
  if [[ -f e2e/probe_completion_order.py ]]; then
    run python3 e2e/probe_completion_order.py --name "$E2E_NAME" --settle-ms 0 --turns 5
  else
    echo 'SKIP completion probe: unavailable in this tag'
  fi
  run e2e/env-down.sh --name "$E2E_NAME"
  E2E_STARTED=false
else
  echo 'SKIP E2E smoke/probe: harness unavailable in this tag'
fi
run scripts/build-fork.sh
echo 'Checks passed. Test the side-by-side Codeg-Fork build, then:'
echo '  scripts/build-selfhost.sh --stage'
echo "  scripts/selfhost/swap.sh --dry-run --staged \"$HOME/Applications/codeg-staged/codeg.app\""
echo 'After review, run swap.sh with the same --staged path without --dry-run.'
echo 'No push, official installation or relay handover was performed.'
