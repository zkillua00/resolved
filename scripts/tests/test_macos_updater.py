"""Packaging health/architecture checks; no compilation, signing, or network access."""
import importlib.util
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import Mock, patch


spec = importlib.util.spec_from_file_location(
    'verify_macos_updater', Path(__file__).resolve().parents[1] / 'verify-macos-updater.py')
verifier = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verifier)

artifact_spec = importlib.util.spec_from_file_location(
    'cargo_artifact', Path(__file__).resolve().parents[1] / 'cargo-artifact.py')
artifacts = importlib.util.module_from_spec(artifact_spec)
artifact_spec.loader.exec_module(artifacts)


class ArtifactTests(unittest.TestCase):
    def setUp(self):
        self.manifest = Path('/project/macos/updater/Cargo.toml')
        self.path = '/project/target/macos-updater/aarch64-apple-darwin/debug/resolved-updater'
        self.artifact = {
            'reason': 'compiler-artifact', 'manifest_path': str(self.manifest),
            'target': {'kind': ['bin'], 'name': 'resolved-updater'},
            'profile': {'test': False}, 'executable': self.path,
        }
        self.finished = {'reason': 'build-finished', 'success': True}

    def test_uses_reported_target_qualified_path(self):
        path = artifacts.executable_path(
            [self.artifact, self.finished], self.manifest, 'resolved-updater')
        self.assertEqual(path, Path(self.path))

    def test_does_not_fall_back_to_a_guessed_binary(self):
        for messages in [[], [self.finished], [self.artifact],
                         [self.artifact, {'reason': 'build-finished', 'success': False}]]:
            with self.subTest(messages=messages), self.assertRaises(ValueError):
                artifacts.executable_path(messages, self.manifest, 'resolved-updater')

    def test_rejects_ambiguous_targets(self):
        with self.assertRaises(ValueError):
            artifacts.executable_path(
                [self.artifact, self.artifact, self.finished], self.manifest, 'resolved-updater')

    def test_rejects_test_binary(self):
        self.artifact['profile']['test'] = True
        with self.assertRaises(ValueError):
            artifacts.executable_path(
                [self.artifact, self.finished], self.manifest, 'resolved-updater')

    def test_rejects_wrong_package(self):
        self.artifact['manifest_path'] = '/another/project/Cargo.toml'
        with self.assertRaises(ValueError):
            artifacts.executable_path(
                [self.artifact, self.finished], self.manifest, 'resolved-updater')


class HealthTests(unittest.TestCase):
    def setUp(self):
        self.response = {
            'protocol_version': 1,
            'kind': 'health',
            'name': 'resolved-updater',
            'app_version': '0.12.4',
            'build_version': '0.12.4.abcdef123456',
            'build_number': '42',
            'os': 'macos',
            'arch': 'arm64',
            'feed_url': 'https://apiworkbench.dev/downloads.json',
            'capabilities': ['health', 'download'],
            'installation_enabled': False,
        }

    def verify(self, stdout):
        verifier.verify_health(stdout, '0.12.4', '0.12.4.abcdef123456', '42', 'arm64')

    def test_expected_identity_and_download_capability(self):
        self.verify(json.dumps(self.response) + '\n')

    def test_each_wrong_metadata_field_is_rejected(self):
        for key in self.response:
            with self.subTest(key=key):
                wrong = dict(self.response, **{key: 'incorrect'})
                with self.assertRaises(ValueError):
                    self.verify(json.dumps(wrong) + '\n')

    def test_installation_must_remain_disabled(self):
        self.response['installation_enabled'] = True
        with self.assertRaises(ValueError):
            self.verify(json.dumps(self.response) + '\n')

    def test_protocol_number_cannot_be_boolean(self):
        self.response['protocol_version'] = True
        with self.assertRaises(ValueError):
            self.verify(json.dumps(self.response) + '\n')

    def test_health_response_framing(self):
        response = json.dumps(self.response)
        for stdout in ['', response, response + '\n\n', response + '\n' + response + '\n',
                       'diagnostic\n' + response + '\n', 'not JSON\n']:
            with self.subTest(stdout=stdout), self.assertRaises(ValueError):
                self.verify(stdout)

    def test_missing_or_extra_fields_are_rejected(self):
        del self.response['build_number']
        with self.assertRaises(ValueError):
            self.verify(json.dumps(self.response) + '\n')
        self.response['build_number'] = '42'
        self.response['unrecognized'] = True
        with self.assertRaises(ValueError):
            self.verify(json.dumps(self.response) + '\n')


class WorkspaceTests(unittest.TestCase):
    def test_desktop_and_mcp_are_allowed(self):
        verifier.verify_workspace({'packages': [
            {'name': 'api-tester'}, {'name': 'resolved-mcp'}]})

    def test_helper_workspace_membership_is_rejected(self):
        with self.assertRaises(ValueError):
            verifier.verify_workspace({'packages': [{'name': 'resolved-updater'}]})

    def test_dependency_on_helper_is_rejected(self):
        with self.assertRaises(ValueError):
            verifier.verify_workspace({'packages': [{
                'name': 'api-tester', 'dependencies': [{'name': 'resolved-updater'}]}]})


class ExecutableTests(unittest.TestCase):
    def setUp(self):
        self.binary = Mock()
        self.binary.is_symlink.return_value = False
        self.binary.is_file.return_value = True

    def test_expected_native_architectures(self):
        for arch, macho in [('arm64', 'arm64'), ('x64', 'x86_64')]:
            with self.subTest(arch=arch), patch.object(verifier.os, 'access', return_value=True), \
                    patch.object(verifier.subprocess, 'run') as run:
                run.return_value.stdout = macho + '\n'
                verifier.verify_executable(self.binary, arch)

    def test_wrong_architecture_is_rejected(self):
        with patch.object(verifier.os, 'access', return_value=True), \
                patch.object(verifier.subprocess, 'run') as run:
            run.return_value.stdout = 'x86_64\n'
            with self.assertRaises(ValueError):
                verifier.verify_executable(self.binary, 'arm64')

    def test_non_executable_is_rejected_before_launch(self):
        with patch.object(verifier.os, 'access', return_value=False), \
                patch.object(verifier.subprocess, 'run') as run:
            with self.assertRaises(ValueError):
                verifier.verify_executable(self.binary, 'arm64')
            run.assert_not_called()

    def test_symlink_is_rejected(self):
        self.binary.is_symlink.return_value = True
        with self.assertRaises(ValueError):
            verifier.verify_executable(self.binary, 'arm64')

    def test_health_failure_is_not_accepted(self):
        with patch.object(verifier, 'verify_executable'), \
                patch.object(verifier.subprocess, 'run',
                             side_effect=subprocess.CalledProcessError(2, 'health')):
            with self.assertRaises(subprocess.CalledProcessError):
                verifier.verify(self.binary, '0.12.4', '0.12.4', '42', 'arm64')


if __name__ == '__main__':
    unittest.main()
