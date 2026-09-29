# Tootie host metrics to Cortex

This Compose project runs the official OpenTelemetry Collector as a separate producer. It reads tootie's CPU, memory, and load once per minute and sends OTLP/HTTP metrics to Cortex. It does not modify the Cortex server stack or instrument other application containers. Docker logs and host heartbeats continue through the existing Cortex agent.

The image is pinned to the OpenTelemetry Collector 0.161.0 multi-platform digest. The [host metrics receiver](https://github.com/open-telemetry/opentelemetry-collector-contrib/blob/main/receiver/hostmetricsreceiver/README.md) reads the mounted host `/proc` and `/sys`; the container shares the host PID and network namespaces so those metrics describe tootie. The [OTLP HTTP exporter](https://github.com/open-telemetry/opentelemetry-collector/blob/main/exporter/otlphttpexporter/README.md) sends protobuf to `127.0.0.1:3100/v1/metrics` and appends the signal path automatically.

## Install

1. Copy `compose.yaml` and `config.yaml` into a task-owned directory on tootie. The live installation uses `/mnt/cache/appdata/cortex-otel-producer`.
2. Create a private `.env` there with `CORTEX_OTLP_AUTH_HEADER="Bearer <current CORTEX_TOKEN>"`. The token is the server's machine-ingest `CORTEX_TOKEN`, not its REST `CORTEX_API_TOKEN`. Keep the directory owned by the operator and `.env` mode `0600`; never commit, print, or copy the token to this repository. Update it when Cortex rotates that credential.
3. Run `docker compose config --quiet`, then `docker compose up -d --no-deps collector` from that directory. No listener is published by this project.
4. Confirm the container is running and its logs have no export errors. On the Cortex host, query `otel_metric_points` for `hostname='tootie'` and `service_name='cortex-hostmetrics'`; require at least two distinct `received_at` batches about a minute apart and check the latest timestamp. The stored resource attributes also carry `host.name`, `service.name`, and `service.namespace`.

The exporter explicitly sets `compression: none`. Collector 0.161.0 defaults to gzip, while the deployed Cortex receiver currently returns HTTP 400 for compressed metric requests. Remove this override only after Cortex has passed a live default-compression OTLP test. The private `.env` is read by Docker Compose and injected into the Collector process, not mounted as a file.

## Stop or roll back

From the installation directory, run `docker compose down`. This stops only this producer. Keep its config and private `.env` for recovery. Cortex does not delete metrics already received when the producer stops.

## Scope

This producer supplies **metrics only**. It does not generate traces or application OTLP logs. Axon and Labby currently do not configure OTLP exporters; Glances on tootie does not expose a Prometheus `/metrics` endpoint. Claude Code supports OTLP logs and metrics, but macpoo's CLI OAuth session was expired during the 2026-09-29 check, so no Claude OTLP producer was left enabled. General host metric browsing through Cortex's REST/MCP UI still needs a bounded read endpoint; Agent Observatory's telemetry route requires a run key.
