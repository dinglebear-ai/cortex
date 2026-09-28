---
name: version-check
description: Use when checking whether the running cortex Docker container matches the local Compose image. Use when the user asks whether cortex is current, stale, deployed, updated, running the latest plugin image, or needs a restart/recreate after an upgrade. Supports an optional --pull mode for Docker image comparison.
---

# Cortex Version Check

Check whether the active cortex runtime is current before suggesting a restart or redeploy.

## Workflow

1. Run the runtime checker from the plugin root:

   ```bash
   ${CLAUDE_PLUGIN_ROOT}/scripts/check-runtime-current.sh <optional-args>
   # Note: if this script is missing, run: docker inspect $(docker compose ps -q cortex) --format '{{.Image}}'
   ```

   If `CLAUDE_PLUGIN_ROOT` is not available or the script is missing, stop with
   a clear failure: "plugin root unavailable; cannot run bundled version
   checker". Do not guess a source checkout path. Suggest the user run
   `redeploy` to restore plugin artifacts.

2. Pass `--pull` only when the user asks to refresh Docker image metadata first. In Docker mode, `--pull` pulls the compose image before comparing the running container image ID to the local compose image ID. Without `--pull`, Docker mode only proves the running container matches the image already present in the local cache.

3. Report these fields:
   - Mode detected: `docker`
   - Running version or image ID
   - Installed binary hash or local compose image ID
   - Verdict: `CURRENT`, `STALE`, or `FAIL`

4. If the verdict is `STALE`, include the exact fix printed by the checker. If the verdict is `FAIL`, include what the checker could not inspect.

## Output

Keep the answer compact and evidence-based. Do not infer freshness from source files alone; use the checker output as the source of truth.
