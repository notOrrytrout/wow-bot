#!/bin/zsh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DATA="$ROOT/wow-bot-data"
LOGS="$DATA/logs"
TMP_ROOT="$ROOT/tmp"
TMP_LOGS="$TMP_ROOT/logs"
SERVICE_DIR="$TMP_ROOT/service"
PLIST="$SERVICE_DIR/com.wowbot.supervisor.plist"
BOOTSTRAP_PLIST="$SERVICE_DIR/com.wowbot.supervisor.$(id -u).plist"
LABEL="com.wowbot.supervisor"
DOMAIN="gui/$(id -u)"
CONFIG="$(cd "$ROOT/.." && pwd)/config.toml"
SUPERVISOR="$ROOT/target/debug/wow-bot-supervisor"
WORKER="$ROOT/target/debug/wow-bot-worker"
SERVICE_BIN_DIR="$TMP_ROOT/bin-$(id -u)"
SERVICE_SUPERVISOR="$SERVICE_BIN_DIR/wow-bot-supervisor"
SERVICE_WORKER="$SERVICE_BIN_DIR/wow-bot-worker"
CONSOLE_LOG="$TMP_LOGS/supervisor-console.log"

mkdir -p "$LOGS" "$TMP_LOGS" "$SERVICE_DIR"
chmod 700 "$TMP_ROOT" "$TMP_LOGS" "$SERVICE_DIR"
export TMPDIR="$TMP_ROOT"

write_plist() {
  /usr/bin/python3 - "$PLIST" "$ROOT" "$SERVICE_SUPERVISOR" "$SERVICE_WORKER" "$DATA" "$CONSOLE_LOG" "$LABEL" "$CONFIG" <<'PY'
import os
import plistlib
import sys

path, root, supervisor, worker, data, console_log, label, config = sys.argv[1:]
job = {
    "Label": label,
    "ProgramArguments": [supervisor, "--worker-bin", worker, "--config", config],
    "WorkingDirectory": root,
    "EnvironmentVariables": {
        "RUST_LOG": "info",
        "WOW_BOT_HOME": data,
        "TMPDIR": os.path.dirname(os.path.dirname(console_log)),
        "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
    },
    "KeepAlive": True,
    "RunAtLoad": True,
    "ThrottleInterval": 10,
    "ProcessType": "Background",
    "StandardOutPath": console_log,
    "StandardErrorPath": console_log,
}
with open(path, "wb") as output:
    plistlib.dump(job, output)
PY
}

loaded() {
  launchctl print "$DOMAIN/$LABEL" >/dev/null 2>&1
}

start_service() {
  if loaded; then
    if running; then
      print "wow-bot service is already running."
      status_service
      return
    fi
    print "Removing the loaded wow-bot service that is not running."
    launchctl bootout "$DOMAIN/$LABEL"
  fi
  (cd "$ROOT" && cargo build -p wow-bot-supervisor -p wow-bot-worker)
  mkdir -p "$SERVICE_BIN_DIR"
  cp "$SUPERVISOR" "$SERVICE_SUPERVISOR"
  cp "$WORKER" "$SERVICE_WORKER"
  write_plist
  cp "$PLIST" "$BOOTSTRAP_PLIST"
  launchctl bootstrap "$DOMAIN" "$BOOTSTRAP_PLIST"
  wait_for_startup
  print "Started wow-bot as a background service."
  status_service
}

stop_service() {
  if loaded; then
    launchctl bootout "$DOMAIN/$LABEL"
    print "Stopped wow-bot service."
  else
    print "wow-bot service is not loaded."
  fi
}

status_service() {
  if ! loaded; then
    print "wow-bot service is stopped."
    return 1
  fi
  local details pid worker_count
  details="$(launchctl print "$DOMAIN/$LABEL")"
  print "$details" | sed -n '1,18p'
  pid="$(print "$details" | awk '/^[[:space:]]*pid = / {print $3; exit}')"
  if [[ -z "$pid" ]]; then
    print -u2 "wow-bot service is loaded but not running."
    print "$details" | rg 'state =|last exit code =|last terminating signal =' | head -n 4 >&2
    print -u2 "console log: $CONSOLE_LOG"
    [[ ! -f "$CONSOLE_LOG" ]] || tail -n 30 "$CONSOLE_LOG" >&2
    return 1
  fi
  worker_count="$( (pgrep -P "$pid" -f 'wow-bot-worker' || true) | wc -l | tr -d ' ')"
  print "worker processes: $worker_count"
  print "runtime log: $LOGS/wow-bot.log"
  print "console log: $CONSOLE_LOG"
}

running() {
  launchctl print "$DOMAIN/$LABEL" 2>/dev/null | awk '/^[[:space:]]*pid = / { found=1 } END { exit !found }'
}

wait_for_startup() {
  local attempt
  for attempt in {1..20}; do
    if running; then
      sleep 2
      if running; then
        return 0
      fi
      break
    fi
    sleep 0.5
  done
  print -u2 "wow-bot supervisor did not stay running; launchd status and console output follow."
  status_service || true
  return 1
}

case "${1:-status}" in
  start) start_service ;;
  stop) stop_service ;;
  restart) stop_service; sleep 2; start_service ;;
  status) status_service ;;
  *) print -u2 "usage: $0 {start|stop|restart|status}"; exit 2 ;;
esac
