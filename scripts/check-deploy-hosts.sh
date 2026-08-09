#!/usr/bin/env bash
set -euo pipefail

variables=(DEV_HOST EDGE_HOST WINDOWS_WSL_HOST LAPTOP_WSL_HOST NAS_HOST EDGE_SOURCE_IP CORTEX_OTLP_ENDPOINT)
for name in "${variables[@]}"; do
  value="${!name:-}"
  if [ -z "$value" ]; then
    echo "$name must be set by a deployment-local hosts.env" >&2
    exit 2
  fi
  if [[ "$value" == *example.invalid* || "$value" == 192.0.2.* || "$value" == 198.51.100.* || "$value" == 203.0.113.* ]]; then
    echo "$name still contains the non-routable example sentinel" >&2
    exit 2
  fi
done
echo '[deploy-hosts] OK - deployment-local values replaced every sentinel'
