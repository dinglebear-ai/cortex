#!/usr/bin/env python3
"""Redact bounded records without buffering an entire command's output."""
import pathlib
import re
import sys

MAX_LINE = 1024 * 1024
MAX_SECRETS = 1024 * 1024
PATTERNS = [
    (r'(Authorization:\s*(?:Bearer|Basic)\s+)[^\s"}]+', r'\1[REDACTED]'),
    (r'((?:token|secret|password|passwd|api[_-]?key|access[_-]?key|client[_-]?secret|refresh[_-]?token|session|cookie)[=:]\s*)[^\s&;]+', r'\1[REDACTED]'),
    (r'("(?:token|secret|password|passwd|api[_-]?key|access[_-]?key|client[_-]?secret|refresh[_-]?token|session|cookie)"\s*:\s*")[^"]+"', r'\1[REDACTED]"'),
    (r'(https?://)[^/@\s]+:[^/@\s]+@', r'\1[REDACTED]@'),
    (r'(-----BEGIN (?:[A-Z0-9 ]+ )?PRIVATE KEY-----).*', r'\1 [REDACTED]'),
]

def redact_stream(source, output, secrets, max_bytes):
    total = 0
    while True:
        line = source.readline(MAX_LINE + 1)
        if not line:
            return
        total += len(line)
        if len(line) > MAX_LINE or total > max_bytes:
            raise ValueError('redaction input exceeds byte budget')
        text = line.decode('utf-8', errors='replace')
        for secret in secrets:
            text = text.replace(secret, '[REDACTED]')
        for pattern, replacement in PATTERNS:
            text = re.sub(pattern, replacement, text, flags=re.I)
        output.write(text)

def main():
    path = pathlib.Path(sys.argv[1])
    raw = b''
    if path.exists():
        with path.open('rb') as registry:
            raw = registry.read(MAX_SECRETS + 1)
        if len(raw) > MAX_SECRETS:
            raise ValueError('secret registry exceeds byte budget')
    secrets = sorted(filter(None, raw.decode('utf-8').splitlines()), key=len, reverse=True)
    redact_stream(sys.stdin.buffer, sys.stdout, secrets, int(sys.argv[2]))

if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
