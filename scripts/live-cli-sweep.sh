#!/usr/bin/env bash
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${1:-$ROOT/target/live-cli-sweep}"
mkdir -p "$OUT_DIR"

SUMMARY="$OUT_DIR/summary.tsv"
: >"$SUMMARY"
printf "name\tmode\texit\tseconds\tlog\n" >>"$SUMMARY"

ENV_FILE="${CORTEX_ENV_FILE:-$HOME/.cortex/.env}"
SERVER="${CORTEX_URL:-http://127.0.0.1:3100}"
TIMEOUT="${CORTEX_SWEEP_TIMEOUT:-120}"
BIN_DIR="${CORTEX_SWEEP_BIN_DIR:-}"
RUN_DEFERRED="${CORTEX_SWEEP_RUN_DEFERRED:-false}"
INTEGRITY_JOB_ID="${CORTEX_SWEEP_INTEGRITY_JOB_ID:-}"
HELPER="$ROOT/scripts/live-cli-sweep-helpers.sh"

export CORTEX_SWEEP_HELPER="$HELPER"

if ! (
  set -a
  if [[ -f "$ENV_FILE" ]]; then
    # shellcheck source=/dev/null
    source "$ENV_FILE"
  fi
  set +a
  if [[ -n "$BIN_DIR" ]]; then
    export PATH="$BIN_DIR:$PATH"
  fi
  "$HELPER" preflight "$ROOT" "$SERVER"
); then
  echo "live CLI sweep aborted before cases: runtime parity preflight failed" >&2
  exit 1
fi

run_case() {
  local name="$1"
  local mode="$2"
  local cmd="$3"
  local safe_name log start end status
  safe_name="$(printf '%s' "$name" | tr -c 'A-Za-z0-9_.-' '_')"
  log="$OUT_DIR/$safe_name.log"
  start="$(date +%s)"
  (
    cd "$ROOT" || exit 99
    set -a
    if [[ -f "$ENV_FILE" ]]; then
      # shellcheck source=/dev/null
      source "$ENV_FILE"
    fi
    set +a
    export CORTEX_USE_HTTP=true
    export CORTEX_URL="$SERVER"
    export CORTEX_SWEEP_TMP="${CORTEX_SWEEP_TMP:-$OUT_DIR/tmp}"
    if [[ -n "$BIN_DIR" ]]; then
      export PATH="$BIN_DIR:$PATH"
    fi
    mkdir -p "$CORTEX_SWEEP_TMP"
    timeout "$TIMEOUT" bash -c "$cmd"
  ) >"$log" 2>&1
  status=$?
  end="$(date +%s)"
  printf "%s\t%s\t%s\t%s\t%s\n" "$name" "$mode" "$status" "$((end-start))" "$log" >>"$SUMMARY"
}

run_case "top.version" "read" "cortex --version"
run_case "top.help" "read" "cortex --help"
run_case "nested.help.sessions" "read" "cortex sessions --help"
run_case "nested.help.db" "read" "cortex db --help"
run_case "completion.zsh" "read" "cortex completions zsh >/dev/null"
run_case "complete.top" "read" "cortex __complete actions"

run_case "search" "read" "cortex search --json --grep cortex --limit 1"
run_case "filter" "read" "cortex filter --json --limit 1"
run_case "tail" "read" "cortex tail --json -n 1"
run_case "hosts.list" "read" "cortex hosts --json"
run_case "hosts.sources" "read" "cortex hosts sources --json --limit 5"
run_case "hosts.silent" "read" "cortex hosts silent --json --silent-minutes 1"
run_case "apps" "read" "cortex apps --json --limit 5"
run_case "entity" "read" "cortex entity host devhost --json"
run_case "graph.status" "local-read" "export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; cortex graph status --json"
run_case "graph.around" "read" "cortex graph around host devhost --json --limit 5"
run_case "graph.explain" "read" "cortex graph explain host devhost --json --max-chains 5"
run_case "graph.evidence.expected-arg" "parse" "cortex graph evidence 1 --json"
if [[ "$RUN_DEFERRED" == "true" ]]; then
  run_case "graph.rebuild.expected-deferred" "expected-deferred" "export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; cortex graph rebuild --json"
else
  run_case "graph.rebuild.expected-deferred" "expected-deferred" "echo 'deferred: set CORTEX_SWEEP_RUN_DEFERRED=true to rebuild the production graph'"
fi

run_case "analysis.errors" "read" "cortex analysis errors --json"
run_case "analysis.incident" "local-read" "export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; cortex analysis incident --json"
run_case "analysis.patterns" "read" "cortex analysis patterns --json"
run_case "analysis.anomalies" "read" "cortex analysis anomalies --json --recent-minutes 60 --baseline-minutes 120"
run_case "analysis.compare" "read" "cortex analysis compare --json"
run_case "correlate.events" "read" "cortex correlate events --json"
run_case "correlate.state" "read" "cortex correlate state --json"
run_case "correlate.topic" "read" "cortex correlate topic --json cortex --since 1h --limit 5"
run_case "state.host" "read" "cortex state host --json"
run_case "state.fleet" "read" "cortex state fleet --json"
run_case "state.clock-skew" "read" "cortex state clockskew --json --limit 5"
run_case "stats.summary" "read" "cortex stats summary --json"
run_case "stats.ingest-rate" "read" "cortex stats ingestrate --json"
run_case "status" "local-read" "export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; cortex status --json"
run_case "timeline" "read" "cortex timeline --json --since 1h"

run_case "sessions.list" "read" "cortex sessions --json --limit 2"
run_case "sessions.search" "read" "cortex sessions search --json cortex --limit 2"
run_case "sessions.abuse" "read" "cortex sessions abuse --json --limit 2"
run_case "sessions.correlate" "read" "cortex sessions correlate --json --ai-query cortex --limit 2 --events-per-anchor 1"
run_case "sessions.blocks" "read" "cortex sessions blocks --json --limit 2"
run_case "sessions.context" "read" "cortex sessions context --json /home/jmagar/workspace/cortex --limit 2"
run_case "sessions.tools" "read" "cortex sessions tools --json"
run_case "sessions.projects" "read" "cortex sessions projects --json"
run_case "sessions.checkpoints" "read" "cortex sessions checkpoints --json --limit 2"
run_case "sessions.errors" "read" "cortex sessions errors --json --limit 2"
run_case "sessions.prune-checkpoints.dry-run" "admin" "cortex sessions prunecheckpoints --json --dry-run --limit 2"
run_case "sessions.doctor" "local-read" "export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; cortex sessions doctor --json"
run_case "sessions.watch-status" "local-read" "export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; cortex sessions watchstatus --json"
run_case "sessions.smoke-watch.expected-deferred" "expected-fail" "export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; cortex sessions smokewatch --json"
run_case "sessions.similar" "read" "cortex sessions similar --json cortex --limit 2"
run_case "sessions.incident-context" "read" "cortex sessions incidentcontext --json"
run_case "sessions.incidents" "read" "cortex sessions incidents --json --limit 2"
run_case "sessions.investigate" "read" "cortex sessions investigate --json --limit 1 --max-bytes 2048"
run_case "sessions.assess.expected-empty" "expected-fail" "export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; cortex sessions assess nonexistent-incident --json --dry-run"
run_case "sessions.llm-invocations" "admin" "cortex sessions llminvocations --json --limit 2"
run_case "sessions.skills" "read" "cortex sessions skills --json --limit 2"
run_case "sessions.skills.backfill.dry-run" "local-mutation-dry-run" "export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; cortex sessions skills backfill --json --dry-run --limit 2"
run_case "sessions.skill-incidents" "read" "cortex sessions skillincidents --json --limit 2"
run_case "sessions.skill-investigate" "read" "cortex sessions skillinvestigate imagegen --json --limit 1"
run_case "sessions.skill-assess" "local-read" 'export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; skill="$(cortex sessions skillincidents --json --limit 1 | python3 -c "import json,sys; d=json.load(sys.stdin); xs=d.get(\"incidents\") or []; print(xs[0].get(\"skill_name\", \"\") if xs else \"\")")"; test -n "$skill" || { echo "no skill incident available for assessment"; exit 0; }; cortex sessions skillassess "$skill" --json --no-llm --limit 1'
run_case "sessions.mcp-events" "read" "cortex sessions mcpevents --json --limit 2"
run_case "sessions.mcp-events.backfill.dry-run" "local-mutation-dry-run" "export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; cortex sessions mcpevents backfill --json --dry-run --limit 2"
run_case "sessions.mcp-incidents" "read" "cortex sessions mcpincidents --json --limit 2"
run_case "sessions.mcp-investigate" "read" 'server="$(cortex sessions mcpincidents --json --limit 1 | python3 -c "import json,sys; d=json.load(sys.stdin); xs=d.get(\"incidents\") or []; print((xs[0].get(\"mcp_server\") or xs[0].get(\"server\") or \"\") if xs else \"\")")"; test -n "$server" || { echo "no MCP incident available for investigation"; exit 0; }; cortex sessions mcpinvestigate "$server" --json --limit 1'
run_case "sessions.mcp-assess" "local-read" 'export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; server="$(cortex sessions mcpincidents --json --limit 1 | python3 -c "import json,sys; d=json.load(sys.stdin); xs=d.get(\"incidents\") or []; print((xs[0].get(\"mcp_server\") or xs[0].get(\"server\") or \"\") if xs else \"\")")"; test -n "$server" || { echo "no MCP incident available for assessment"; exit 0; }; cortex sessions mcpassess "$server" --json --no-llm --limit 1'
run_case "sessions.hook-events" "read" "cortex sessions hookevents --json --limit 2"
run_case "sessions.hook-incidents" "read" "cortex sessions hookincidents --json --limit 2"
run_case "sessions.hook-investigate" "read" 'hook="$(cortex sessions hookincidents --json --limit 1 | python3 -c "import json,sys; d=json.load(sys.stdin); xs=d.get(\"incidents\") or []; print((xs[0].get(\"hook_name\") or xs[0].get(\"hook_event\") or \"\") if xs else \"\")")"; test -n "$hook" || { echo "no hook incident available for investigation"; exit 0; }; cortex sessions hookinvestigate "$hook" --json --limit 1'
run_case "sessions.hooks-backfill.dry-run" "local-mutation-dry-run" "export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; cortex sessions hooksbackfill --json --dry-run --limit 2"

run_case "assess.skill" "local-read" 'export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; skill="$(cortex sessions skillincidents --json --limit 1 | python3 -c "import json,sys; d=json.load(sys.stdin); xs=d.get(\"incidents\") or []; print(xs[0].get(\"skill_name\", \"\") if xs else \"\")")"; test -n "$skill" || { echo "no skill incident available for assessment"; exit 0; }; cortex assess skill "$skill" --json --no-llm --limit 1'
run_case "assess.abuse" "local-read" 'export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; incident="$(cortex sessions incidents --json --limit 1 | python3 -c "import json,sys; d=json.load(sys.stdin); xs=d.get(\"incidents\") or []; print(xs[0].get(\"incident_id\", \"\") if xs else \"\")")"; test -n "$incident" || { echo "no abuse incident available for assessment"; exit 0; }; cortex assess abuse --incident-id "$incident" --json --no-llm'
run_case "assess.mcp" "local-read" 'export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; server="$(cortex sessions mcpincidents --json --limit 1 | python3 -c "import json,sys; d=json.load(sys.stdin); xs=d.get(\"incidents\") or []; print((xs[0].get(\"mcp_server\") or xs[0].get(\"server\") or \"\") if xs else \"\")")"; test -n "$server" || { echo "no MCP incident available for assessment"; exit 0; }; cortex assess mcp "$server" --json --no-llm --limit 1'
run_case "assess.hooks" "local-read" 'export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; hook="$(cortex sessions hookincidents --json --limit 1 | python3 -c "import json,sys; d=json.load(sys.stdin); xs=d.get(\"incidents\") or []; print((xs[0].get(\"hook_name\") or xs[0].get(\"hook_event\") or \"\") if xs else \"\")")"; test -n "$hook" || { echo "no hook incident available for assessment"; exit 0; }; cortex assess hooks "$hook" --json --no-llm --limit 1'

run_case "alerts.signatures.list" "read" "cortex alerts signatures list --json --limit 2"
export CORTEX_SWEEP_SIGNATURE_HASH
CORTEX_SWEEP_SIGNATURE_HASH="$(python3 -c 'import json,sys; s=open(sys.argv[1]).read(); d=json.loads(s[s.index("{"):]); xs=d.get("signatures") or d.get("items") or d.get("results") or []; print((xs[0].get("signature_hash") or xs[0].get("hash")) if xs else "")' "$OUT_DIR/alerts.signatures.list.log")"
run_case "alerts.signatures.ack-unack" "admin" 'hash="$CORTEX_SWEEP_SIGNATURE_HASH"; test -n "$hash" || { echo "no signature available"; exit 0; }; cortex alerts signatures ack "$hash" --json --notes "live-cli-sweep"; cortex alerts signatures unack "$hash" --json --reason "live-cli-sweep revert"'
run_case "alerts.notifications.recent" "read" "cortex alerts notifications recent --json --limit 2"
run_case "alerts.notifications.test" "admin" "cortex alerts notifications test --json --body live-cli-sweep"

run_case "ingest.inventory.status" "read" "cortex ingest inventory status --json"
run_case "ingest.inventory.refresh" "local-mutation" "export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db CORTEX_INVENTORY_DIR=$HOME/.cortex/inventory; cortex ingest inventory refresh --json"
run_case "ingest.syslog.status" "local-read" "export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; cortex ingest syslog status --json"
run_case "ingest.syslog.test.expected-deferred" "expected-fail" "cortex ingest syslog test"
run_case "ingest.docker.status" "local-read" "export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; cortex ingest docker status --json"
run_case "ingest.docker.sources" "local-read" "export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; cortex ingest docker sources --json"
run_case "ingest.file-tail.list" "admin" "cortex ingest filetail list --json"
run_case "ingest.file-tail.status" "admin" "cortex ingest filetail status --json"
run_case "ingest.file-tail.add-toggle-remove" "admin" 'id="live-cli-sweep"; cortex ingest filetail add /file-tail-root/auth.log --id "$id" --json; cortex ingest filetail disable "$id" --json; cortex ingest filetail enable "$id" --json; cortex ingest filetail remove "$id" --json'
run_case "ingest.shell.user.index" "local-mutation" 'export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; hist="$CORTEX_SWEEP_TMP/history"; printf "echo live-cli-sweep\n" >"$hist"; cortex ingest shell user index "$hist" --json'
run_case "ingest.shell.user.atuin-index" "local-mutation" 'export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; cortex ingest shell user atuinindex --json'
run_case "ingest.shell.agent.wrap-probe" "read" "cortex ingest shell agent wrap --probe"
run_case "ingest.shell.agent.index" "local-mutation" 'export CORTEX_USE_HTTP=false CORTEX_DB_PATH=$HOME/.cortex/data/cortex.db; spool="$CORTEX_SWEEP_TMP/agent-spool.jsonl"; : >"$spool"; chmod 600 "$spool"; cortex ingest shell agent index "$spool" --json'

run_case "heartbeat.agent.once.emit" "read" 'cortex heartbeat agent --once --emit --json --target "$CORTEX_URL" --token "$CORTEX_API_TOKEN" --docker'

run_case "db.status" "read" "cortex db status --json"
run_case "db.status.coord" "read" "cortex db status --json --check-coord"
if [[ -n "$INTEGRITY_JOB_ID" ]]; then
  run_case "db.integrity.quick.background" "admin" 'cortex db integrity status --json "$CORTEX_SWEEP_INTEGRITY_JOB_ID" | jq -e '\''(.status == "running" or .status == "done")'\'' >/dev/null'
else
  run_case "db.integrity.quick.background" "admin" 'started="$(cortex db integrity --json --quick --background)"; job_id="$(jq -er .job_id <<<"$started")"; cortex db integrity status --json "$job_id" | jq -e '\''(.status == "running" or .status == "done")'\'' >/dev/null'
fi
run_case "db.checkpoint.passive" "admin" "cortex db checkpoint passive --json"
run_case "db.vacuum.incremental" "admin" "cortex db vacuum --json --pages 1"
if [[ "$RUN_DEFERRED" == "true" ]]; then
  run_case "db.backup" "expected-long" "cortex db backup --json"
else
  run_case "db.backup" "expected-long" "echo 'deferred: set CORTEX_SWEEP_RUN_DEFERRED=true to run a full production backup'"
fi

run_case "compose.status" "read" "cortex compose status --json"
run_case "compose.doctor" "read" "cortex compose doctor --json"
run_case "compose.logs" "read" "cortex compose logs --json --tail 5"
run_case "compose.logs.service" "read" "cortex compose logs cortex --json --tail 5"
run_case "compose.up.dry-run" "mutation-dry-run" "cortex compose up --json --dry-run"
run_case "compose.restart.dry-run" "mutation-dry-run" "cortex compose restart --json --dry-run"
run_case "compose.pull.dry-run" "mutation-dry-run" "cortex compose pull --json --dry-run"
run_case "compose.down.dry-run" "mutation-dry-run" "cortex compose down --json --dry-run --yes"
run_case "compose.config.expected-deferred" "expected-fail" "cortex compose config"
run_case "compose.upgrade.expected-deferred" "expected-fail" "cortex compose upgrade"

run_case "setup.check" "read" "cortex setup check --json"
run_case "setup.plugin-hook.no-repair" "read" "cortex setup pluginhook --json --no-repair"
run_case "config.list" "read" 'cfg="$CORTEX_SWEEP_TMP/config.toml"; printf "[mcp]\nport = 3100\n" >"$cfg"; cortex config list --toml --toml-path "$cfg" --json'
run_case "config.get" "read" 'cfg="$CORTEX_SWEEP_TMP/config.toml"; printf "[mcp]\nport = 3100\n" >"$cfg"; cortex config get mcp.port --toml --toml-path "$cfg" --json'
run_case "config.set" "mutation-temp" 'cfg="$CORTEX_SWEEP_TMP/config.toml"; printf "[mcp]\nport = 3100\n" >"$cfg"; cortex config set test.value live-cli-sweep --toml --toml-path "$cfg" --json'
run_case "config.unset" "mutation-temp" 'cfg="$CORTEX_SWEEP_TMP/config.toml"; printf "[test]\nvalue = \"live-cli-sweep\"\n" >"$cfg"; cortex config unset test.value --toml --toml-path "$cfg" --json'

echo "summary: $SUMMARY"
awk -F '\t' 'NR == 1 { next } $2 ~ /expected-/ { print }' "$SUMMARY" >"$OUT_DIR/expected.tsv"
awk -F '\t' 'NR == 1 { next } $2 !~ /expected-/ && $3 != 0 { print }' "$SUMMARY" >"$OUT_DIR/failures.tsv"
echo "expected: $OUT_DIR/expected.tsv"
echo "failures: $OUT_DIR/failures.tsv"
