#!/usr/bin/env bash
# The single search evidence page must include TCP, receipted, and sentinel rows.
concurrency_bounds_valid() {
  local workers="$1" each="$2"
  [[ "$workers" =~ ^[1-8]$ && "$each" =~ ^[1-9][0-9]{0,2}$ ]] || return 1
  (( each <= 200 && (workers + 1) * each + 1 <= 1000 ))
}
