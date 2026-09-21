"""Packaging health/architecture checks; no compilation, signing, or network access."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
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
            'capabilities': ['health', 'download', 'verify', 'verify-host', 'install', 'recover'],
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


class BoundedCommandTests(unittest.TestCase):
    # Only spawn Python; never codesign, spctl, keychains, or a real app/helper.
    def test_captures_both_streams(self):
        self.assertEqual(verifier.run_bounded([
            sys.executable, '-c', 'import sys; print("out"); print("err", file=sys.stderr)']),
            ('out\n', 'err\n'))

    def test_rejects_output_overflow(self):
        with self.assertRaises(ValueError):
            verifier.run_bounded([sys.executable, '-c', 'print("x" * 65537)'])

    def test_rejects_timeout(self):
        with self.assertRaises(subprocess.TimeoutExpired):
            verifier.run_bounded([sys.executable, '-c', 'import time; time.sleep(5)'],
                                 timeout=0.1)

    def test_rejects_failed_exit(self):
        with self.assertRaises(subprocess.CalledProcessError):
            verifier.run_bounded([sys.executable, '-c', 'raise SystemExit(7)'])


class DeveloperIDTests(unittest.TestCase):
    def test_team_parser_cardinality_and_injection(self):
        self.assertEqual(verifier.parse_team('Other=value\nTeamIdentifier=ABCDE12345\n'),
                         'ABCDE12345')
        for metadata in ['', 'TeamIdentifier=not set\n',
                         'TeamIdentifier=ABCDE12345\nTeamIdentifier=ABCDE12345\n',
                         'TeamIdentifier=abcde12345\n', 'TeamIdentifier=ABCDE123456\n',
                         'TeamIdentifier=ABCDE12345 \n',
                         'TeamIdentifier=ABCDE12345" or true\n']:
            with self.subTest(metadata=metadata), self.assertRaises(ValueError):
                verifier.parse_team(metadata)

    def test_requirements_pin_certificate_type_identifiers_and_team(self):
        for identifier in ['dev.apitester.desktop', 'dev.apitester.desktop.updater']:
            requirement = verifier.developer_id_requirement(identifier, 'ABCDE12345')
            self.assertTrue(requirement.startswith('='), 'codesign must parse text, not a requirement filename')
            self.assertIn('anchor apple generic', requirement)
            self.assertIn(f'identifier "{identifier}"', requirement)
            self.assertIn('certificate 1[field.1.2.840.113635.100.6.2.6] exists', requirement)
            self.assertIn('certificate leaf[field.1.2.840.113635.100.6.1.13] exists', requirement)
            self.assertIn('certificate leaf[subject.OU] = "ABCDE12345"', requirement)
        for team in ['ABCDE12345" or true', 'abcde12345', '']:
            with self.assertRaises(ValueError):
                verifier.developer_id_requirement('dev.apitester.desktop', team)
        with self.assertRaises(ValueError):
            verifier.developer_id_requirement('dev.apitester.desktop" or true')

    @patch.object(verifier, 'run_bounded')
    def test_validates_signature_before_deriving_team(self, run):
        run.side_effect = [('', ''), ('', ''), ('', 'TeamIdentifier=ABCDE12345\n'),
                           ('', ''), ('', '')]
        self.assertEqual(verifier.verify_developer_id(Path('/Resolved.app'), Path('/helper')),
                         'ABCDE12345')
        commands = [call.args[0] for call in run.call_args_list]
        self.assertIn('--verify', commands[0])
        self.assertIn('.6.1.13', commands[0][-2])
        self.assertIn('--deep', commands[1])
        self.assertIn('--display', commands[2])
        self.assertIn('subject.OU] = "ABCDE12345"', commands[3][-2])
        self.assertIn('dev.apitester.desktop.updater', commands[4][-2])

    @patch.object(verifier, 'run_bounded')
    def test_expected_team_mismatch(self, run):
        run.side_effect = [('', ''), ('', ''), ('', 'TeamIdentifier=ABCDE12345\n')]
        with self.assertRaises(ValueError):
            verifier.verify_developer_id(Path('/Resolved.app'), Path('/helper'), 'ZZZZZ99999')
        self.assertEqual(run.call_count, 3)

    @patch.object(verifier, 'run_bounded')
    def test_signature_failures_stop_verification(self, run):
        for failure_index in [0, 1, 3, 4]:
            results = [('', ''), ('', ''), ('', 'TeamIdentifier=ABCDE12345\n'),
                       ('', ''), ('', '')]
            results[failure_index] = subprocess.CalledProcessError(1, 'codesign')
            run.reset_mock()
            run.side_effect = results
            with self.assertRaises(subprocess.CalledProcessError):
                verifier.verify_developer_id(Path('/Resolved.app'), Path('/helper'))
            self.assertEqual(run.call_count, failure_index + 1)


class HostVerificationTests(unittest.TestCase):
    def setUp(self):
        self.response = {
            'protocol_version': 1, 'kind': 'host_verified', 'installation_enabled': False,
            'team_id': 'ABCDE12345', 'app_version': '0.12.4', 'build_number': '42',
        }

    def verify(self, response):
        verifier.verify_host_response(response, '0.12.4', '42', 'ABCDE12345')

    def test_valid_response(self):
        self.verify(json.dumps(self.response) + '\n')

    def test_wrong_fields_and_types(self):
        for key, value in [('protocol_version', True), ('protocol_version', 1.0),
                           ('kind', 'health'), ('installation_enabled', True),
                           ('installation_enabled', 0), ('team_id', 'ZZZZZ99999'),
                           ('app_version', '0.12.5'), ('build_number', 42)]:
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                self.verify(json.dumps(dict(self.response, **{key: value})) + '\n')

    def test_missing_extra_fields_and_framing(self):
        response = json.dumps(self.response)
        missing = dict(self.response)
        del missing['team_id']
        for output in ['', response, response + '\n\n', response + '\n' + response + '\n',
                       'not JSON\n', json.dumps(missing) + '\n',
                       json.dumps(dict(self.response, extra=True)) + '\n']:
            with self.subTest(output=output), self.assertRaises(ValueError):
                self.verify(output)

    @patch.object(verifier, 'run_bounded')
    def test_bundled_command_and_failure(self, run):
        run.return_value = (json.dumps(self.response) + '\n', '')
        verifier.verify_host(Path('/Resolved.app/Contents/Helpers/resolved-updater'),
                             '0.12.4', '0.12.4', '42', 'ABCDE12345')
        self.assertEqual(run.call_args.args[0], [
            '/Resolved.app/Contents/Helpers/resolved-updater',
            '--protocol-version', '1', 'verify-host'])
        for error in [subprocess.CalledProcessError(1, 'verify-host'),
                      subprocess.TimeoutExpired('verify-host', 120),
                      ValueError('output limit')]:
            run.side_effect = error
            with self.assertRaises(type(error)):
                verifier.verify_host(Path('/helper'), '0.12.4', '0.12.4', '42', 'ABCDE12345')

    @patch.object(verifier, 'run_bounded')
    def test_nightly_does_not_run_host_verification(self, run):
        with self.assertRaises(ValueError):
            verifier.verify_host(Path('/helper'), '0.12.4', '0.12.4.nightly', '42', 'ABCDE12345')
        run.assert_not_called()


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
