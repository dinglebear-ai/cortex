# Host metrics to Cortex

This Compose project runs the official OpenTelemetry Collector as a separate producer. It reads host CPU, memory, and load once per minute and sends OTLP/HTTP metrics to Cortex. CPU and memory utilization gauges are enabled for direct trends alongside the default cumulative usage metrics. It does not modify the Cortex server stack or instrument other application containers. Docker logs and host heartbeats can continue through an existing Cortex agent.

The image is pinned to the OpenTelemetry Collector 0.161.0 multi-platform digest. The [host metrics receiver](https://github.com/open-telemetry/opentelemetry-collector-contrib/blob/main/receiver/hostmetricsreceiver/README.md) reads the mounted host `/proc` and `/sys`; the container shares the host PID and network namespaces so those metrics describe its deployment host. The [OTLP HTTP exporter](https://github.com/open-telemetry/opentelemetry-collector/blob/main/exporter/otlphttpexporter/README.md) sends protobuf to `127.0.0.1:3100/v1/metrics` and appends the signal path automatically.

## Install

1. Copy `compose.yaml` and `config.yaml` into a directory dedicated to this producer on the Cortex host.
2. Create a private `.env` there with `CORTEX_OTLP_AUTH_HEADER="Bearer <current CORTEX_TOKEN>"` and `CORTEX_OTLP_HOST_NAME=<source-hostname>`. The token is the server's machine-ingest `CORTEX_TOKEN`, not its REST `CORTEX_API_TOKEN`. Keep `.env` mode `0600`; never commit, print, or copy the token to this repository. Update it when Cortex rotates that credential.
3. Run `docker compose config --quiet`, then `docker compose up -d --no-deps collector` from that directory. No listener is published by this project.
4. Confirm the container is running and its logs have no export errors. On the Cortex host, query `otel_metric_points` for `hostname='<source-hostname>'` and `service_name='cortex-hostmetrics'`; require at least two distinct `received_at` batches about a minute apart and check the latest timestamp. The stored resource attributes also carry `host.name`, `service.name`, and `service.namespace`.

The exporter explicitly sets `compression: none`. Collector 0.161.0 defaults to gzip, while the deployed Cortex receiver currently returns HTTP 400 for compressed metric requests. Remove this override only after Cortex has passed a live default-compression OTLP test. The private `.env` is read by Docker Compose and injected into the Collector process, not mounted as a file.

## Stop or roll back

From the installation directory, run `docker compose down`. This stops only this producer. Keep its config and private `.env` for recovery. Cortex does not delete metrics already received when the producer stops.

## Scope

This producer supplies **metrics only**. It does not generate traces or application OTLP logs. Those require instrumented applications and separate exporter setup. Generic host metric browsing through Cortex's REST/MCP UI still needs a bounded read endpoint; Agent Observatory's telemetry route requires a run key.
