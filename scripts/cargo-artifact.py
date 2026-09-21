#!/usr/bin/env python3
"""Select the executable reported by this Cargo build, never a guessed/stale target path."""
import argparse
import json
from pathlib import Path


def executable_path(messages, manifest, name):
    manifest = manifest.resolve()
    matches = []
    finished = False
    for message in messages:
        if message.get('reason') == 'build-finished':
            finished = message.get('success') is True
        if (message.get('reason') == 'compiler-artifact'
                and message.get('target', {}).get('name') == name
                and message.get('target', {}).get('kind') == ['bin']
                and message.get('profile', {}).get('test') is False
                and Path(message['manifest_path']).resolve() == manifest
                and message.get('executable')):
            matches.append(Path(message['executable']))
    if not finished or len(matches) != 1 or not matches[0].is_absolute():
        raise ValueError(f'Expected one {name} executable from a successful Cargo build; got {matches!r}')
    return matches[0]


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('messages', type=Path)
    parser.add_argument('manifest', type=Path)
    parser.add_argument('name')
    args = parser.parse_args()
    try:
        with args.messages.open(encoding='utf-8') as source:
            messages = [json.loads(line) for line in source]
        print(executable_path(messages, args.manifest, args.name))
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.exit(1, f'error: {error}\n')
