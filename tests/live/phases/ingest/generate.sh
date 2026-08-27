#!/usr/bin/env bash
set -euo pipefail
type="$1" count="$2" max_records="$3" max_bytes="$4" deadline="$5" output="$6"
[[ "$count" =~ ^[0-9]+$ && "$max_records" =~ ^[1-9][0-9]*$ && "$max_bytes" =~ ^[1-9][0-9]*$ && "$deadline" =~ ^[1-9][0-9]*$ ]]
(( count <= max_records )) || { echo 'record budget exceeded' >&2; exit 2; }
start=$(date +%s); : >"$output"
for ((i=1;i<=count;i++)); do
  (( $(date +%s)-start < deadline )) || { echo 'deadline exceeded' >&2; exit 3; }
  printf '%s-%04d\n' "$type" "$i" >>"$output"
  (( $(wc -c <"$output") <= max_bytes )) || { echo 'byte budget exceeded' >&2; exit 4; }
done
jq -cn --arg type "$type" --argjson count "$count" --argjson bytes "$(wc -c <"$output")" '{type:$type,count:$count,bytes:$bytes}'
