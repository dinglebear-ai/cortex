# Cortex plugin

This package connects agent clients to Cortex and ships log-intelligence skills plus the first-run `install-cortex` workflow.

## First-class install

```sh
npx skills add dinglebear-ai/cortex --skill install-cortex
```

Then invoke `$install-cortex`. It can provision the one Cortex server in a fleet or configure a client-only machine against an existing server. The workflow delegates runtime setup to the canonical Cortex installer and `cortex setup repair`.

The complete Claude plugin remains described by the repository's root `.claude-plugin/plugin.json`. Plugin configuration supports server/client mode, bearer or Google OAuth, separate service credentials, syslog listener settings, storage policy, and remote server URLs.
