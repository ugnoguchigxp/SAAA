#!/usr/bin/env bash
# Butler live profile: start SAAA with every butler-loop switch ON.
#
# Product defaults stay OFF. This script flips the developer's own data
# directory and process environment only. It does not change source defaults.
#
# Switches:
#   SAAA_MEMORY_ENABLED=1            Steward reducer + World compose (env)
#   schedule_runtime.enabled=1       tick loop does work (SQLite)
#   situation.runtime.enabled=true   Situation monitor -> MEETING scene -> TTS hold (SQLite)
#   coding.enabled                   verified only (must already be true in Settings)
#   active Goal                      registered in the app UI (Coding -> Steward panel); verified only
#
# Usage:
#   scripts/butler-live.sh            # flip switches, print status, start `bun start`
#   scripts/butler-live.sh --status   # print status only
#   scripts/butler-live.sh --off      # restore schedule/situation to OFF (env is per-process)
set -euo pipefail

DB="${SAAA_DB_PATH:-$HOME/Library/Application Support/com.saaa.desktop/saaa.sqlite3}"
MODE="${1:-start}"

if [[ ! -f "$DB" ]]; then
  echo "database not found: $DB" >&2
  echo "start SAAA once normally so the data directory is created, then rerun." >&2
  exit 1
fi

if pgrep -x saaa >/dev/null 2>&1 || pgrep -f 'target/debug/saaa' >/dev/null 2>&1; then
  echo "SAAA appears to be running; stop it first (single-writer lock)." >&2
  exit 1
fi

now_iso() { date -u +%Y-%m-%dT%H:%M:%S.000Z; }

flip() {
  local enabled="$1" # 1 or 0
  local json_bool; [[ "$enabled" == "1" ]] && json_bool=true || json_bool=false
  cp "$DB" "$DB.bak-butler-$(date +%Y%m%d%H%M%S)"
  sqlite3 "$DB" "
    UPDATE schedule_runtime SET enabled=$enabled WHERE id=1;
    UPDATE settings_documents
       SET value_json=json_set(value_json,'\$.enabled',json('$json_bool')), updated_at='$(now_iso)'
     WHERE namespace='situation.runtime' AND key='default';
  "
}

status() {
  local sched sit coding goals ws
  sched=$(sqlite3 "$DB" "select enabled||'/'||calendar_enabled from schedule_runtime where id=1")
  sit=$(sqlite3 "$DB" "select json_extract(value_json,'\$.enabled') from settings_documents where namespace='situation.runtime' and key='default'")
  coding=$(sqlite3 "$DB" "select json_extract(value_json,'\$.enabled')||' profile='||json_extract(value_json,'\$.profile') from coding_settings where id=1" 2>/dev/null || echo "n/a")
  goals=$(sqlite3 "$DB" "select count(*) from steward_goals where status='active'" 2>/dev/null || echo "n/a")
  ws=$(sqlite3 "$DB" "select group_concat(path, ', ') from coding_workspaces" 2>/dev/null || echo "")
  cat <<EOF
== butler switches ==
SAAA_MEMORY_ENABLED        : ${SAAA_MEMORY_ENABLED:-<unset>}   (set to 1 by this script when starting)
schedule_runtime enabled   : $sched   (enabled/calendar_enabled; calendar needs Google OAuth in Settings)
situation.runtime enabled  : $sit      (1 = monitor on; macOS Accessibility permission required)
coding enabled             : $coding
active steward goals       : $goals    (register in app: Coding -> Steward panel)
coding workspaces          : ${ws:-<none>}
EOF
  if [[ "$goals" == "0" ]]; then
    cat <<'EOF'

No active Goal. After the app starts, open the Coding view -> Steward panel and register one, e.g.:
  summary            : bbs のテストを監視する
  success condition  : テストが通ること
  verifier           : test_report_obtained
  operations         : read_test
  budget runs        : 3
  notify             : both
Then say「テストを確認して」in chat (exact trigger in the current build).
EOF
  fi
}

case "$MODE" in
  --status) status ;;
  --off)    flip 0; status ;;
  start)
    flip 1
    export SAAA_MEMORY_ENABLED=1
    status
    echo
    echo "starting SAAA with SAAA_MEMORY_ENABLED=1 ..."
    cd "$(dirname "$0")/.."
    exec bun start
    ;;
  *) echo "usage: $0 [--status|--off]" >&2; exit 2 ;;
esac
