#!/usr/bin/env python3
"""Compare the Rust managed dotenv encoder with Compose's offline config parser."""
from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT: Path = Path(__file__).resolve().parents[1]
VALUES: list[str] = ['$TOKEN', '${TOKEN}', '$$dollar', 'abc # comment', '#leading', '"double"',
          "'single'", 'back\\slash', 'trailing\\', '\\"both', "\\'both", ' spaces ',
          'unicode 🚀', 'dollar $TOKEN # quote " slash \\']


def main() -> None:
    docker = shutil.which('docker')
    if not docker:
        raise SystemExit('docker compose is required for the offline dotenv compatibility check')
    with tempfile.TemporaryDirectory(prefix='cortex-dotenv-') as directory:
        root = Path(directory)
        source = root / 'encode.rs'
        source.write_text(f'#[path = {json.dumps(str(ROOT / "src/setup/dotenv.rs"))}] mod dotenv;\n'
                          'fn main() { for (i, value) in std::env::args().skip(1).enumerate() '
                          '{ println!("VALUE_{}={}", i, dotenv::encode(&value)); } }\n')
        binary = root / 'encode'
        subprocess.run(['rustc', '--edition', '2024', str(source), '-o', str(binary)], check=True,
                       capture_output=True, text=True)
        encoded = subprocess.run([str(binary), *VALUES], capture_output=True, text=True, check=True).stdout
        env_file = root / 'fixture.env'
        env_file.write_text(encoded)
        compose = root / 'compose.yml'
        compose.write_text(f'services:\n  fixture:\n    image: scratch\n    env_file:\n      - {env_file}\n')
        result = subprocess.run([docker, 'compose', '--env-file', str(env_file), '-f', str(compose),
                                 'config', '--format', 'json'], capture_output=True, text=True,
                                check=True, env=dict(os.environ, TOKEN='must-not-expand'))
        environment = json.loads(result.stdout)['services']['fixture']['environment']
        # Compose config escapes literal dollars in its reusable rendered model.
        for i, value in enumerate(VALUES):
            actual = environment[f'VALUE_{i}'].replace('$$', '$')
            assert actual == value, (i, value, actual)
        print(f'Compose dotenv compatibility: {len(VALUES)} literal values passed (no containers started)')


if __name__ == '__main__':
    main()
