#!/bin/zsh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DATA="$ROOT/wow-bot-data"
LOGS="$DATA/logs"
SERVICE_DIR="$DATA/service"
PLIST="$SERVICE_DIR/com.wowbot.supervisor.plist"
BOOTSTRAP_PLIST="${TMPDIR:-/tmp}/com.wowbot.supervisor.$(id -u).plist"
LABEL="com.wowbot.supervisor"
DOMAIN="gui/$(id -u)"
SUPERVISOR="$ROOT/target/debug/wow-bot-supervisor"
WORKER="$ROOT/target/debug/wow-bot-worker"
SERVICE_BIN_DIR="${TMPDIR:-/tmp}/wow-bot-bin-$(id -u)"
SERVICE_SUPERVISOR="$SERVICE_BIN_DIR/wow-bot-supervisor"
SERVICE_WORKER="$SERVICE_BIN_DIR/wow-bot-worker"
CONSOLE_LOG="${TMPDIR:-/tmp}/wow-bot-supervisor.$(id -u).log"

mkdir -p "$LOGS" "$SERVICE_DIR"

write_plist() {
  /usr/bin/python3 - "$PLIST" "$ROOT" "$SERVICE_SUPERVISOR" "$SERVICE_WORKER" "$DATA" "$CONSOLE_LOG" "$LABEL" <<'PY'
import plistlib
import sys

path, root, supervisor, worker, data, console_log, label = sys.argv[1:]
job = {
    "Label": label,
    "ProgramArguments": [supervisor, "--worker-bin", worker],
    "WorkingDirectory": root,
    "EnvironmentVariables": {
        "RUST_LOG": "info",
        "WOW_BOT_HOME": data,
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
    print "wow-bot service is already loaded."
    status_service
    return
  fi
  (cd "$ROOT" && cargo build -p wow-bot-supervisor -p wow-bot-worker)
  mkdir -p "$SERVICE_BIN_DIR"
  cp "$SUPERVISOR" "$SERVICE_SUPERVISOR"
  cp "$WORKER" "$SERVICE_WORKER"
  write_plist
  cp "$PLIST" "$BOOTSTRAP_PLIST"
  launchctl bootstrap "$DOMAIN" "$BOOTSTRAP_PLIST"
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
  if [[ -n "$pid" ]]; then
    worker_count="$( (pgrep -P "$pid" -f 'wow-bot-worker' || true) | wc -l | tr -d ' ')"
    print "worker processes: $worker_count"
  fi
  print "runtime log: $LOGS/wow-bot.log"
  print "console log: $CONSOLE_LOG"
}

case "${1:-status}" in
  start) start_service ;;
  stop) stop_service ;;
  restart) stop_service; sleep 2; start_service ;;
  status) status_service ;;
  *) print -u2 "usage: $0 {start|stop|restart|status}"; exit 2 ;;
esac
