#!/usr/bin/env python3
"""Reject a stale release matrix before any upgrade workload starts."""
import json
import pathlib
import re
import sys
import tomllib

def validate(root):
    current = tomllib.loads((root / 'Cargo.toml').read_text())['package']['version']
    matrix = json.loads((root / 'tests/live/contracts/releases/compatibility.json').read_text())
    if matrix['candidate'] != current:
        raise ValueError(f'upgrade candidate {matrix["candidate"]} does not match Cargo version {current}; refresh predecessor pins')
    version = lambda text: tuple(map(int, text.split('.')))
    for entry in matrix['supported'].values():
        if version(entry['version']) >= version(current):
            raise ValueError('upgrade predecessor must precede candidate')
        if not re.fullmatch(r'.+@sha256:[0-9a-f]{64}', entry['image']):
            raise ValueError('upgrade image must have an immutable digest')
        if not entry['origin'].endswith(':v' + entry['version']):
            raise ValueError('upgrade origin/version mismatch')

if __name__ == '__main__':
    try:
        validate(pathlib.Path(sys.argv[1]))
    except (ValueError, KeyError) as error:
        raise SystemExit(str(error))
