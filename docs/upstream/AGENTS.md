# External integrations — Cortex

Follow the root [AGENTS.md](../../AGENTS.md). Cortex stores and queries its own evidence; it is not merely an API wrapper. That does not mean it has no optional outbound integrations or credentials.

## Source families

Syslog arrives over UDP/TCP. Host-local agents forward supported Docker, file, transcript, shell, and command evidence through the implemented ingest endpoints. OTLP logs/metrics/traces use HTTP/protobuf. Local scanner/watch paths support AI transcript files. Inventory collectors use configured SSH or service APIs and persist a normalized inventory cache.

See [architecture.md](../architecture.md), [ADDING_SOURCES.md](../ADDING_SOURCES.md), and [SETUP.md](../SETUP.md). Legacy central pull Docker ingestion is a compatibility path, not the default alternative to the host-local agent.

## Optional outbound dependencies

Inventory collectors may use SSH credentials or service-specific API credentials. Notifications use Apprise. OAuth uses the configured Google/lab-auth integration. LLM-backed CLI assessments use the configured backend and its authorization. These integrations are optional/configuration-dependent; their credentials are not interchangeable with MCP, REST, or machine-ingest bearer tokens. Consult [CONFIG.md](../CONFIG.md), [OAUTH.md](../OAUTH.md), and [SECURITY.md](../SECURITY.md).

## Trust boundaries

Treat syslog fields, transcript contents, API payloads, and claimed hostnames as untrusted input. A socket peer address is an observed transport address, not cryptographic identity. Preserve source provenance, redaction, schema validation, bounded reads/writes, retry limits, and checkpoint acknowledgement. Use parameterized SQL and the existing FTS query-validation path.

Do not broaden filesystem discovery roots, disable SSH host-key verification, expose a listener, or alter remote credentials simply to make a test pass. Production integration checks require explicit target and operation authorization.
