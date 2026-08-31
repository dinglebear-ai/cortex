#!/usr/bin/env bash

live_timeout() {
  local seconds="$1"; shift
  local marker
  marker="$(mktemp "${TMPDIR:-/tmp}/cortex-live-timeout.XXXXXX")"; rm -f "$marker"
  "$@" & local command_pid=$!
  # Poll instead of a single long sleep. On macOS, terminating the watcher
  # shell does not reliably reap its `sleep` child, causing successful short
  # commands to block until the full timeout. This exits within one second of
  # normal completion and also works when the target is a shell function.
  (
    local elapsed=0
    while kill -0 "$command_pid" 2>/dev/null && (( elapsed < seconds )); do
      sleep 1
      ((elapsed+=1))
    done
    kill -0 "$command_pid" 2>/dev/null || exit 0
    touch "$marker"
    kill -TERM "$command_pid" 2>/dev/null || exit 0
    sleep 1
    kill -KILL "$command_pid" 2>/dev/null || true
  ) & local watcher_pid=$!
  local status=0
  wait "$command_pid" || status=$?
  kill "$watcher_pid" 2>/dev/null || true
  wait "$watcher_pid" 2>/dev/null || true
  if [[ -e "$marker" ]]; then status=124; fi
  rm -f "$marker"
  return "$status"
}

live_run_bounded() {
  local seconds="$1" stdout="$2" stderr="$3"; shift 3
  [[ "$seconds" =~ ^[1-9][0-9]*$ ]] || { live_die "invalid timeout"; return; }
  live_budget_add processes 1
  live_secure_dir "$(dirname "$stdout")"
  local pipe_dir out_pipe err_pipe status out_filter err_filter
  pipe_dir="$(mktemp -d "${LIVE_RUN_ROOT}/.capture.XXXXXX")"
  out_pipe="$pipe_dir/stdout"; err_pipe="$pipe_dir/stderr"
  mkfifo "$out_pipe" "$err_pipe"; chmod 600 "$out_pipe" "$err_pipe"
  live_redact_stream <"$out_pipe" >"$stdout" & out_filter=$!
  live_redact_stream <"$err_pipe" >"$stderr" & err_filter=$!
  if live_timeout "$seconds" live_sanitized_env "$@" >"$out_pipe" 2>"$err_pipe"; then status=0; else status=$?; fi
  wait "$out_filter"; wait "$err_filter"
  chmod 600 "$stdout" "$stderr"
  rm -f "$out_pipe" "$err_pipe"; rmdir "$pipe_dir"
  live_event command "$(jq -cn --arg status "$status" --arg timeout "$seconds" '{status:($status|tonumber),timeout_seconds:($timeout|tonumber)}')"
  return "$status"
}

live_poll() {
  local timeout="$1" interval="$2" description="$3"; shift 3
  local started now attempts=0
  started="$(date +%s)"
  while true; do
    ((attempts+=1))
    live_budget_add poll_attempts 1
    "$@" && { live_event poll "$(jq -cn --arg d "$description" --argjson a "$attempts" '{description:$d,attempts:$a,result:"ready"}')"; return 0; }
    now="$(date +%s)"
    if (( now - started >= timeout )); then
      live_event poll "$(jq -cn --arg d "$description" --argjson a "$attempts" '{description:$d,attempts:$a,result:"timeout"}')"
      return 124
    fi
    sleep "$interval"
  done
}
