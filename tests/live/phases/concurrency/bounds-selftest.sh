#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/bounds.sh"
concurrency_bounds_valid 4 30
concurrency_bounds_valid 8 111
concurrency_bounds_valid 4 199
for pair in '8 112' '8 200' '4 200' '9 1' '1 0' '1 99999999999999999999999999999999'; do
  read -r workers each <<<"$pair"
  if concurrency_bounds_valid "$workers" "$each"; then
    echo "unexpectedly accepted workload: $pair" >&2
    exit 1
  fi
done
echo 'concurrency bounds selftest: PASS'
