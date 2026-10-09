#!/usr/bin/env bash
# shellcheck disable=SC2153
unset CODEG_DATA_DIR CODEG_HOME CODEG_STATIC_DIR CODEG_ACP_TMP_ROOT CODEG_TOKEN CODEG_PORT CODEG_HOST
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
# shellcheck source=scripts/selfhost/common.sh
source "$HERE/common.sh"
[[ $# == 1 ]] || die 'Worker requires a prepared transaction manifest'
MANIFEST=$1
eval "$(support exports "$MANIFEST")"
[[ "${BACKUP_COMPLETE:-false}" == true && -d "$BACKUP_DIR/.lock" ]] || die 'Incomplete backup or missing lock'
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
CHANGED=false
DATA_CHANGED=false
STOP_ATTEMPTED=false
RESULT=FAIL

wait_stopped() {
  local deadline=$((SECONDS + $1))
  while [[ -n "$(support pids "$CURRENT")" ]]; do
    [[ $SECONDS -lt $deadline ]] || return 1
    sleep 1
  done
}

stop_app() {
  [[ -n "$(support pids "$CURRENT")" ]] || return 0
  support single-target "$MANIFEST" || return 1
  log "Graceful quit bundle=$BUNDLE_ID (45s)"
  # The Apple Event itself has a timeout, not just the subsequent process poll.
  osascript -e "with timeout of 5 seconds" -e "tell application id \"$BUNDLE_ID\" to quit" -e 'end timeout' || log 'Quit event failed/timed out; checking exact executable'
  wait_stopped 40 && return 0
  log 'Escalating to TERM; waiting 15s'
  support signal "$CURRENT" TERM || return 1
  wait_stopped 15 && return 0
  log 'Escalating to KILL'
  support signal "$CURRENT" KILL || return 1
  wait_stopped 5
}

register_app() {
  run xattr -dr com.apple.quarantine "$CURRENT" || return 1
  run "$LSREGISTER" -f "$CURRENT"
}

relaunch() {
  local which=$1 deadline pid owners required=$REQUIRE_PORT
  if [[ "$which" == target && -n "$RESTORE_SNAPSHOT" ]]; then required=$RESTORE_REQUIRE_PORT; fi
  support verify "$MANIFEST" "$which" || return 1
  if [[ "$NO_RELAUNCH" == true ]]; then log 'Runtime verification deferred (--no-relaunch)'; return; fi
  run open -a "$CURRENT" || return 1
  deadline=$((SECONDS + 60))
  while [[ $SECONDS -lt $deadline ]]; do
    for pid in $(support pids "$CURRENT"); do
      if [[ "$required" == true ]]; then
        owners=$(lsof -nP -iTCP:"$PORT" -sTCP:LISTEN -t 2>/dev/null || true)
        [[ $'\n'"$owners"$'\n' == *$'\n'"$pid"$'\n'* ]] || continue
      fi
      support verify "$MANIFEST" "$which" || return 1
      log "RELAUNCH OK pid=$pid executable=$CURRENT/Contents/MacOS/codeg port=$PORT required=$required"
      return
    done
    sleep 1
  done
  log 'Relaunch verification timed out after 60s'
  return 1
}

restore_data() {
  local snapshot=$1
  log "EXPLICIT DATA RESTORE from $snapshot (app stopped)"
  # Preserve pre-operation snapshots; delete stale WAL/SHM together with old data.
  DATA_CHANGED=true
  run rsync -a --delete "$snapshot/data/" "$DATA_DIR/" || return 1
  run rsync -a --delete "$snapshot/home/" "$HOME_DIR/"
}

recover() {
  log 'AUTO-ROLLBACK BEGIN (failed transaction retained)'
  stop_app || return 1
  if $CHANGED; then
    if [[ -e "$CURRENT" ]]; then run mv "$CURRENT" "$CURRENT.failed-$ID" || return 1; fi
    run mv "$OLD_APP" "$CURRENT" || return 1
  fi
  if $DATA_CHANGED; then restore_data "$DATA_SNAPSHOT" || return 1; fi
  register_app || return 1
  relaunch original || return 1
  log 'AUTO-ROLLBACK OK'
}

finish() {
  local rc=$?
  trap - EXIT INT TERM
  if [[ $rc != 0 ]]; then
    log "FAIL action=$ACTION exit=$rc"
    # Failure before rename can still leave the original app stopped.
    if $STOP_ATTEMPTED; then
      if ! recover; then log "AUTO-ROLLBACK FAILED: preserve $APP_BACKUP and inspect $LOG"; fi
    fi
  fi
  printf '%s\n' "$RESULT" > "$JOB_DIR/status"
  rmdir "$BACKUP_DIR/.lock" || true
  log "FINAL $RESULT action=$ACTION"
  if [[ -f "$JOB_DIR/job.plist" ]]; then
    # All state is flushed before self-bootout (launchd may terminate this shell).
    launchctl bootout "gui/$(id -u)/$LABEL" >/dev/null 2>&1 || true
  fi
  exit "$rc"
}
trap finish EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
log "WORKER pid=$$ ppid=$PPID action=$ACTION"
sleep 5
support verify "$MANIFEST" original
STOP_ATTEMPTED=true
stop_app
# Prepare on the destination volume before moving the old app; mv is then atomic.
run ditto "$SOURCE" "$NEW_APP"
run codesign --verify --deep --strict "$NEW_APP"
support verify "$MANIFEST" target "$NEW_APP"
[[ ! -e "$OLD_APP" && ! -L "$OLD_APP" ]] || die 'Replaced app path already exists'
run mv "$CURRENT" "$OLD_APP"
CHANGED=true
run mv "$NEW_APP" "$CURRENT"
if [[ -n "$RESTORE_SNAPSHOT" ]]; then restore_data "$RESTORE_SNAPSHOT"; fi
register_app
relaunch target
run rm -rf "$OLD_APP" || log "Could not fully remove redundant replaced app: $OLD_APP"
RESULT=OK
