#!/usr/bin/env python3
"""Verify a built/bundled updater's architecture and side-effect-free health protocol."""
import argparse
import json
import os
from pathlib import Path
import re
import selectors
import subprocess
import time


def run_bounded(command, timeout=30, limit=65536):
    """Bound combined native output while draining both pipes without deadlock."""
    with subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE) as process:
        output = [bytearray(), bytearray()]
        deadline = time.monotonic() + timeout
        try:
            with selectors.DefaultSelector() as selector:
                selector.register(process.stdout, selectors.EVENT_READ, 0)
                selector.register(process.stderr, selectors.EVENT_READ, 1)
                while selector.get_map():
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise subprocess.TimeoutExpired(command, timeout)
                    for key, _ in selector.select(remaining):
                        chunk = os.read(key.fileobj.fileno(), 8192)
                        if not chunk:
                            selector.unregister(key.fileobj)
                            continue
                        output[key.data].extend(chunk)
                        if sum(map(len, output)) > limit:
                            raise ValueError('Native verification output exceeded limit')
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise subprocess.TimeoutExpired(command, timeout)
            status = process.wait(timeout=remaining)
            if status:
                raise subprocess.CalledProcessError(status, command)
            return tuple(bytes(part).decode('utf-8', errors='strict') for part in output)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()


def validate_team(team):
    if not isinstance(team, str) or not re.fullmatch(r'[A-Z0-9]{10}', team):
        raise ValueError('Expected exactly one 10-character uppercase alphanumeric team ID')
    return team


def parse_team(metadata):
    teams = [line[len('TeamIdentifier='):] for line in metadata.splitlines()
             if line.startswith('TeamIdentifier=')]
    if len(teams) != 1:
        raise ValueError('Signature metadata must contain exactly one TeamIdentifier')
    return validate_team(teams[0])


def developer_id_requirement(identifier, team=None):
    if identifier not in ('dev.apitester.desktop', 'dev.apitester.desktop.updater'):
        raise ValueError('Unexpected signing identifier')
    requirement = (
        f'=anchor apple generic and identifier "{identifier}" '
        'and certificate 1[field.1.2.840.113635.100.6.2.6] exists '
        'and certificate leaf[field.1.2.840.113635.100.6.1.13] exists'
    )
    if team is not None:
        requirement += f' and certificate leaf[subject.OU] = "{validate_team(team)}"'
    return requirement


def verify_developer_id(bundle, binary, expected_team=None):
    if expected_team is not None:
        validate_team(expected_team)

    def check(path, identifier, team=None):
        run_bounded(['/usr/bin/codesign', '--verify', '--strict', '--all-architectures',
                     '-R', developer_id_requirement(identifier, team), str(path)])

    # Never derive trust from unvalidated display metadata.
    check(bundle, 'dev.apitester.desktop')
    # Validate nested seals separately: an app identifier requirement must not
    # be recursively applied to code with its own helper identifier.
    run_bounded(['/usr/bin/codesign', '--verify', '--deep', '--strict', '--all-architectures', str(bundle)])
    stdout, stderr = run_bounded(['/usr/bin/codesign', '--display', '--verbose=4', str(bundle)])
    team = parse_team(stdout + stderr)
    if expected_team is not None and team != expected_team:
        raise ValueError('Developer ID team does not match expected team')
    check(bundle, 'dev.apitester.desktop', team)
    check(binary, 'dev.apitester.desktop.updater', team)
    return team


def verify_host_response(stdout, app_version, build_number, team):
    validate_team(team)
    lines = stdout.splitlines()
    if len(lines) != 1 or not stdout.endswith('\n'):
        raise ValueError('Host verification must return one newline-terminated JSON response')
    expected = {
        'protocol_version': 1, 'kind': 'host_verified', 'installation_enabled': False,
        'team_id': team, 'app_version': app_version, 'build_number': build_number,
    }
    if json.dumps(json.loads(lines[0]), sort_keys=True) != json.dumps(expected, sort_keys=True):
        raise ValueError('Host verification identity or trust flag mismatch')


def verify_host(binary, app_version, build_version, build_number, team):
    if app_version != build_version:
        raise ValueError('Host verification is supported only for stable releases')
    stdout, _ = run_bounded([str(binary), '--protocol-version', '1', 'verify-host'], timeout=120)
    verify_host_response(stdout, app_version, build_number, team)


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
        'capabilities': ['health', 'download', 'verify', 'verify-host', 'install', 'recover'],
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
    parser.add_argument('--developer-id', action='store_true')
    parser.add_argument('--expected-team')
    parser.add_argument('--verify-host', action='store_true')
    args = parser.parse_args()
    try:
        if (args.expected_team is not None or args.verify_host) and not args.developer_id:
            raise ValueError('--expected-team and --verify-host require --developer-id')
        if args.workspace_metadata:
            verify_workspace(json.loads(args.workspace_metadata.read_text()))
        team = None
        if args.developer_id:
            bundle = args.binary.parent.parent.parent
            if args.binary != bundle / 'Contents/Helpers/resolved-updater':
                raise ValueError('Developer ID verification requires a bundled updater')
            team = verify_developer_id(bundle, args.binary, args.expected_team)
        verify(args.binary, args.app_version, args.build_version, args.build_number,
               args.arch, args.app_executable)
        if args.verify_host:
            verify_host(args.binary, args.app_version, args.build_version, args.build_number, team)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        parser.exit(1, f'error: {error}\n')
    print(f'Verified updater {args.build_version}, build {args.build_number} ({args.arch}; no artifact authorized)')
