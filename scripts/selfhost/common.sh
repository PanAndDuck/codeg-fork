#!/usr/bin/env bash
# shellcheck disable=SC2153
unset CODEG_DATA_DIR CODEG_HOME CODEG_STATIC_DIR CODEG_ACP_TMP_ROOT CODEG_TOKEN CODEG_PORT CODEG_HOST
set -euo pipefail
# Manifest fields are assigned through support.py's shell-quoted exports.

log() { printf '[%s] %s\n' "$(date '+%Y-%m-%dT%H:%M:%S%z')" "$*"; }
die() { log "FAIL: $*" >&2; exit 1; }
run() { printf '+ '; printf '%q ' "$@"; printf '\n'; "$@"; }
support() { python3 "$HERE/support.py" "$@"; }

print_plan() {
  log "DRY-RUN: read-only preflight passed; no directories, logs, backups or jobs created"
  log "Current: $CURRENT; staged/restore source: $SOURCE"
  log "Log: $LOG; manifest: $MANIFEST"
  log "ditto $CURRENT -> $APP_BACKUP (version, authority, cdhash recorded)"
  log "SQLite ONLINE .backup $DATA_DIR/codeg.db -> $DATA_SNAPSHOT/data/codeg.db; integrity_check + SHA256"
  log "rsync $DATA_DIR/ -> $DATA_SNAPSHOT/data/ excluding codeg.db, codeg.db-wal, codeg.db-shm"
  log "rsync $HOME_DIR/ -> $DATA_SNAPSHOT/home/; write sizes/versions manifest"
  log "Copy worker code -> $JOB_DIR; bootstrap gui/$(id -u)/$LABEL (one-shot); wait 5s"
  log "osascript -e 'tell application id \"$BUNDLE_ID\" to quit'; exact executable $CURRENT/Contents/MacOS/codeg"
  log "Wait 45s, TERM exact-path PIDs, wait 15s, KILL exact-path PIDs if needed; never match codeg-mcp or Codeg-Fork"
  log "ditto $SOURCE -> $NEW_APP; verify signature; mv $CURRENT -> $OLD_APP; mv $NEW_APP -> $CURRENT"
  [[ -z "$RESTORE_SNAPSHOT" ]] || log "EXPLICIT DATA RESTORE: replace $DATA_DIR and $HOME_DIR from $RESTORE_SNAPSHOT (pre-restore snapshot retained)"
  log "xattr -dr com.apple.quarantine $CURRENT; lsregister -f $CURRENT"
  if [[ "$NO_RELAUNCH" == false ]]; then
    log "open -a $CURRENT; wait <=60s for exact new executable, conditional port $PORT LISTEN owned by it; verify CodegBuildCommit/cdhash"
  else
    log "--no-relaunch: verify installed signature/CodegBuildCommit only; runtime verification deferred"
  fi
  log "On failure: auto-rollback app (and data if explicitly restored), relaunch previous app; retain logs and failed bundle"
  log "Remove same-volume replaced app only after success; mark status; remove lock; unload launchd job"
}

selfhost_main() {
  local action=$1 source='' restore='' dry=false inline=false no_relaunch=false context
  shift
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --staged|--to)
        [[ $# -ge 2 ]] || die "Missing path after $1"
        [[ "$1" == --staged && "$action" == swap || "$1" == --to && "$action" == rollback ]] || die "Wrong option for $action"
        source=$2; shift 2 ;;
      --restore-data)
        [[ "$action" == rollback && $# -ge 2 ]] || die 'Expected rollback --restore-data <snapshot-id>'
        restore=$2; shift 2 ;;
      --dry-run) dry=true; shift ;;
      --inline) inline=true; shift ;;
      --no-relaunch) no_relaunch=true; shift ;;
      --help|-h)
        echo 'swap.sh --staged <app> [--dry-run] [--inline] [--no-relaunch]'
        echo 'rollback.sh [--to <backup.app>] [--restore-data <snapshot-id>] [--dry-run] [--inline] [--no-relaunch]'
        return ;;
      *) die "Unknown option: $1" ;;
    esac
  done
  [[ "$action" != swap || -n "$source" ]] || die '--staged is required'
  context=$(support plan "$action" "$source" "$restore" "$no_relaunch" "$inline")
  eval "$(printf '%s' "$context" | support exports)"
  if $dry; then print_plan; return; fi

  umask 077
  mkdir -p "$BACKUP_DIR"
  mkdir "$BACKUP_DIR/.lock" || die "Another transaction or stale lock: $BACKUP_DIR/.lock (inspect before removing)"
  # No process is stopped until every backup, including the manifest, is durable.
  trap 'rmdir "$BACKUP_DIR/.lock" 2>/dev/null || true' EXIT
  mkdir -p "$BACKUP_DIR/apps" "$BACKUP_DIR/manifests" "$BACKUP_DIR/logs" "$JOB_DIR" "$DATA_SNAPSHOT/data" "$DATA_SNAPSHOT/home"
  exec > >(tee -a "$LOG") 2>&1
  log "BEGIN $action id=$ID current=$CURRENT source=$SOURCE"
  printf '%s\n' "$context" > "$MANIFEST"
  run ditto "$CURRENT" "$APP_BACKUP"
  run codesign --verify --deep --strict "$APP_BACKUP"
  log "Online SQLite backup (WAL-aware)"
  support backup-db "$DATA_DIR/codeg.db" "$DATA_SNAPSHOT/data/codeg.db"
  run rsync -a --exclude=/codeg.db --exclude=/codeg.db-wal --exclude=/codeg.db-shm "$DATA_DIR/" "$DATA_SNAPSHOT/data/"
  if [[ -d "$HOME_DIR" ]]; then run rsync -a "$HOME_DIR/" "$DATA_SNAPSHOT/home/"; fi
  support complete "$MANIFEST"
  run cp "$HERE/common.sh" "$HERE/support.py" "$HERE/swap-worker.sh" "$JOB_DIR/"
  sync
  if $inline; then
    log 'Running inline in an isolated test environment'
    trap - EXIT
    /bin/bash "$JOB_DIR/swap-worker.sh" "$MANIFEST"
  else
    support plist "$MANIFEST"
    run launchctl bootstrap "gui/$(id -u)" "$JOB_DIR/job.plist"
    # launchd owns the lock from here. The worker's output goes directly to LOG.
    trap - EXIT
    log "SUBMITTED label=$LABEL; log=$LOG; status=$JOB_DIR/status"
  fi
}
