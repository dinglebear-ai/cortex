#!/usr/bin/env bash

live_contract_export() {
  local destination="$1" tmp
  tmp="$(mktemp "${LIVE_RUN_ROOT:?}/.surface-contract.XXXXXX")"
  cargo run --quiet --manifest-path "$LIVE_PROJECT_ROOT/tests/live/surface-exporter/Cargo.toml" >"$tmp" || { rm -f "$tmp"; live_die "compiled SurfaceContract export failed"; return; }
  jq -e '.version >= 1 and (.entries|type=="array" and length>0) and
    (([.entries[].id]|length)==([.entries[].id]|unique|length)) and
    all(.entries[]; (.id|type=="string" and length>0) and (.kind|type=="string" and length>0) and
      (.profiles|type=="array" and all(.[]; IN("smoke","full","storage","soak"))) and
      (.required_cases|type=="array" and length>0 and all(.[]; IN("semantic-positive","validation-negative","authorization"))))' "$tmp" >/dev/null || { rm -f "$tmp"; live_die "invalid compiled SurfaceContract export"; return; }
  chmod 600 "$tmp"; mv "$tmp" "$destination"; live_manifest_seal "$destination"
  LIVE_SURFACE_CONTRACT="$destination"; export LIVE_SURFACE_CONTRACT
}

live_contract_consume() {
  local contract="$1"
  live_manifest_verify "$contract" || return
  jq -e '.version >= 1 and (.entries|length>0) and (([.entries[].id]|length)==([.entries[].id]|unique|length))' "$contract" >/dev/null || { live_die "invalid authoritative SurfaceContract"; return; }
  LIVE_SURFACE_CONTRACT="$contract"; export LIVE_SURFACE_CONTRACT
}

live_capability_ledger() {
  local contract="$1" profile="$2" events ledger="${LIVE_RUN_ROOT:?}/capability-ledger.jsonl" tmp
  events="$(live_event_file)"
  tmp="$(mktemp "${LIVE_RUN_ROOT}/.ledger.XXXXXX")"
  jq -c --arg profile "$profile" '
    .entries[] | select(.profiles|index($profile)) as $surface |
    $surface.required_cases[] | {surface_id:$surface.id,case_kind:.,mandatory:true}
  ' "$contract" >"$tmp.required"
  while IFS= read -r required; do
    local surface case first
    surface="$(jq -r .surface_id <<<"$required")"; case="$(jq -r .case_kind <<<"$required")"
    first="$(jq -n --arg surface "$surface" --arg case "$case" 'first(inputs | select(.kind=="result" and .payload.surface_id==$surface and .payload.case_kind==$case and .payload.attempt_kind=="first_attempt") | .payload) // null' "$events")"
    jq -cn --argjson required "$required" --argjson outcome "$first" '$required + {outcome:$outcome}' >>"$tmp"
  done <"$tmp.required"
  rm -f "$tmp.required"
  chmod 600 "$tmp"; mv "$tmp" "$ledger"
  LIVE_CAPABILITY_LEDGER="$ledger"; export LIVE_CAPABILITY_LEDGER
}

live_ledger_validate() {
  local contract="$1" profile="$2" ledger
  live_capability_ledger "$contract" "$profile" || return
  ledger="$LIVE_CAPABILITY_LEDGER"
  jq -e -n 'reduce inputs as $row (true; . and
    ($row.outcome != null) and
    ($row.outcome.result == "pass" or $row.outcome.result == "fail"))' "$ledger" >/dev/null ||
    { live_die "mandatory capability missing or qualified/skipped: $profile"; return; }
}
