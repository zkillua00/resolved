#!/usr/bin/env python3
"""Verify the standalone MCP executable and package it for a native CI runner."""
import argparse
import json
from pathlib import Path
import subprocess
import tarfile
import zipfile


def package(platform, arch, version):
    root = Path(__file__).resolve().parent.parent
    executable = 'resolved-mcp.exe' if platform == 'windows' else 'resolved-mcp'
    binary = root / 'target' / 'release' / executable
    if platform == 'macos':
        subprocess.run(['codesign', '--force', '--sign', '-', str(binary)], check=True)
        subprocess.run(['codesign', '--verify', '--strict', str(binary)], check=True)

    # These protocol operations never discover or contact a running desktop.
    messages = [
        {'jsonrpc': '2.0', 'id': 1, 'method': 'initialize',
         'params': {'protocolVersion': '2025-06-18', 'capabilities': {},
                    'clientInfo': {'name': 'release-check', 'version': '1'}}},
        {'jsonrpc': '2.0', 'method': 'notifications/initialized'},
        {'jsonrpc': '2.0', 'id': 2, 'method': 'ping'},
    ]
    result = subprocess.run(
        [str(binary)], input=''.join(json.dumps(m) + '\n' for m in messages),
        text=True, capture_output=True, check=True, timeout=15,
    )
    responses = [json.loads(line) for line in result.stdout.splitlines()]
    if len(responses) != 2:
        raise ValueError(f'Expected two MCP responses, got {responses!r}')
    initialized, pong = responses
    if (initialized.get('jsonrpc') != '2.0' or initialized.get('id') != 1
            or initialized.get('result', {}).get('serverInfo')
            != {'name': 'resolved-mcp', 'version': version}
            or initialized.get('result', {}).get('protocolVersion') != '2025-06-18'
            or pong != {'jsonrpc': '2.0', 'id': 2, 'result': {}}):
        raise ValueError(f'MCP handshake or version check failed: {responses!r}')

    dist = root / 'dist'
    dist.mkdir(exist_ok=True)
    name = f'resolved-mcp-{version}-{platform}-{arch}'
    files = [(binary, executable), (root / 'LICENSE', 'LICENSE'),
             (root / 'docs' / 'mcp.md', 'README.md')]
    if platform == 'linux':
        archive = dist / f'{name}.tar.xz'
        with tarfile.open(archive, 'w:xz') as output:
            for source, filename in files:
                output.add(source, arcname=f'{name}/{filename}')
    else:
        archive = dist / f'{name}.zip'
        with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as output:
            for source, filename in files:
                output.write(source, arcname=f'{name}/{filename}')
    print(f'Verified MCP {version}; packaged {archive.name}')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('platform', choices=['macos', 'windows', 'linux'])
    parser.add_argument('arch', choices=['arm64', 'x64'])
    parser.add_argument('version')
    args = parser.parse_args()
    package(args.platform, args.arch, args.version)
