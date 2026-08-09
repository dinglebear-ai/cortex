# Redaction policy

This public repository uses stable, synthetic identifiers when examples or
historical operational records would otherwise expose private infrastructure.

- Role names such as `devhost`, `nashost`, and `edgehost` are pseudonyms, not
  resolvable deployment targets.
- IPv4 examples use the RFC 5737 documentation ranges. They are intentionally
  non-routable and must never be copied into active configuration.
- Public URL examples use `example.com` or the guaranteed-invalid
  `example.invalid` namespace.
- Historical session and report documents preserve event relationships and
  outcomes, but pseudonyms and synthetic addresses mean their commands are
  evidence of what was done, not executable runbooks.
- Public authorship, project ownership, and canonical published identifiers
  such as `ai.dinglebear/cortex` are provenance, not private infrastructure,
  and are retained.

Executable setup instructions must use explicit placeholders or environment
variables and explain where operators obtain the real value. Active Compose,
CI, and release metadata must never route to a documentation address.
