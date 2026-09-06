#!/usr/bin/env python3
"""Resolve release identity without modifying Cargo's SemVer metadata."""
import argparse
import os
from pathlib import Path
import re
import subprocess
import tomllib


def resolve(root, channel, ref):
    package = tomllib.loads((root / 'Cargo.toml').read_text())['package']
    version = package['version']
    if not re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)', version):
        raise ValueError('Cargo version must be MAJOR.MINOR.PATCH')
    locked = tomllib.loads((root / 'Cargo.lock').read_text())['package']
    if not any(p['name'] == package['name'] and p['version'] == version for p in locked):
        raise ValueError('Cargo.toml and Cargo.lock versions disagree')
    if channel == 'version' and ref != f'refs/tags/v{version}':
        raise ValueError(f'version releases require the matching v{version} tag')
    sha = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip()
    display = f'{version}.{sha[:12]}' if channel == 'nightly' else version
    return {'version': display, 'sha': sha,
            'tag': f'nightly-{display}' if channel == 'nightly' else f'v{version}'}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('channel', choices=['nightly', 'version'])
    parser.add_argument('--ref', default=os.environ.get('GITHUB_REF', ''))
    args = parser.parse_args()
    values = resolve(Path(__file__).resolve().parent.parent, args.channel, args.ref)
    output = ''.join(f'{key}={value}\n' for key, value in values.items())
    print(output, end='')
    if os.environ.get('GITHUB_OUTPUT'):
        with open(os.environ['GITHUB_OUTPUT'], 'a') as stream:
            stream.write(output)
