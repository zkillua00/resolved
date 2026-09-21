#!/usr/bin/env python3
"""Verify a built/bundled updater's architecture and side-effect-free health protocol."""
import argparse
import json
import os
from pathlib import Path
import subprocess


def verify_health(stdout, app_version, build_version, build_number, arch):
    lines = stdout.splitlines()
    if len(lines) != 1 or not stdout.endswith('\n'):
        raise ValueError('Updater health must return exactly one newline-terminated JSON response')
    response = json.loads(lines[0])
    expected = {
        'protocol_version': 1,
        'kind': 'health',
        'name': 'resolved-updater',
        'app_version': app_version,
        'build_version': build_version,
        'build_number': build_number,
        'os': 'macos',
        'arch': arch,
        'feed_url': 'https://apiworkbench.dev/downloads.json',
        'capabilities': ['health'],
        'installation_enabled': False,
    }
    # JSON comparison distinguishes booleans from integers (unlike Python ==).
    if json.dumps(response, sort_keys=True) != json.dumps(expected, sort_keys=True):
        raise ValueError(f'Updater protocol, build identity, or capabilities mismatch: {response!r}')


def verify_workspace(metadata):
    packages = metadata['packages']
    if any(package['name'] == 'resolved-updater' for package in packages):
        raise ValueError('The standalone updater must not belong to the desktop workspace')
    if any(dependency['name'] == 'resolved-updater'
           for package in packages for dependency in package.get('dependencies', [])):
        raise ValueError('Desktop packages must not depend on the updater executable')


def verify_executable(binary, arch):
    if binary.is_symlink() or not binary.is_file() or not os.access(binary, os.X_OK):
        raise ValueError(f'Missing executable or invalid executable permissions: {binary}')
    result = subprocess.run(
        ['/usr/bin/lipo', '-archs', str(binary)],
        text=True, capture_output=True, check=True, timeout=15,
    )
    expected = {'arm64': 'arm64', 'x64': 'x86_64'}[arch]
    if result.stdout.split() != [expected]:
        raise ValueError(f'Expected {expected} executable at {binary}, got {result.stdout.strip()!r}')


def verify(binary, app_version, build_version, build_number, arch, app_executable=None):
    verify_executable(binary, arch)
    if app_executable is not None:
        verify_executable(app_executable, arch)
    result = subprocess.run(
        [str(binary), '--protocol-version', '1', 'health'],
        stdin=subprocess.DEVNULL, text=True, capture_output=True, check=True, timeout=15,
    )
    verify_health(result.stdout, app_version, build_version, build_number, arch)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    parser.add_argument('--app-version', required=True)
    parser.add_argument('--build-version', required=True)
    parser.add_argument('--build-number', required=True)
    parser.add_argument('--arch', choices=['arm64', 'x64'], required=True)
    parser.add_argument('--app-executable', type=Path)
    parser.add_argument('--workspace-metadata', type=Path)
    args = parser.parse_args()
    try:
        if args.workspace_metadata:
            verify_workspace(json.loads(args.workspace_metadata.read_text()))
        verify(args.binary, args.app_version, args.build_version, args.build_number,
               args.arch, args.app_executable)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        parser.exit(1, f'error: {error}\n')
    print(f'Verified updater {args.build_version}, build {args.build_number} ({args.arch}; health only)')
