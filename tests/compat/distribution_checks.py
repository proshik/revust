#!/usr/bin/env python3
"""Distribution policy regressions against disposable Git histories and archives."""
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
CHECK = ROOT / 'scripts/check-distribution.py'
NOREPLY = '123+fixture@users.noreply.github.com'


class Distribution(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='revust-distribution-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / 'repo'
        self.repo.mkdir()
        self.env = dict(os.environ, GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_SYSTEM=os.devnull,
                        GIT_AUTHOR_NAME='fixture', GIT_AUTHOR_EMAIL=NOREPLY,
                        GIT_COMMITTER_NAME='fixture', GIT_COMMITTER_EMAIL=NOREPLY)
        self.git('init', '--quiet', '--initial-branch=main', '--template=')
        self.files = {
            'LICENSE': b'Synthetic product licence\n',
            'Cargo.lock': b'Synthetic lockfile\n',
            'README.md': b'Synthetic public product documentation\n',
            'scripts/tool.py': b'print("synthetic")\n',
            'third_party/fixture/LICENSE': b'Copyright fixture@example.test\nSynthetic third-party licence\n',
        }
        for name, data in self.files.items():
            target = self.repo / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
        self.manifest = {'version': 1, 'source': sorted([*self.files, 'distribution.json']),
                         'payload': sorted([*self.files, 'distribution.json'])}
        self.write_manifest()
        self.commit()
        self.artifacts = self.root / 'artifacts'
        self.artifacts.mkdir()

    def git(self, *args, env=None):
        return subprocess.check_output(['git', *args], cwd=self.repo,
                                       env=env or self.env, stderr=subprocess.PIPE).decode().strip()

    def write_manifest(self):
        (self.repo / 'distribution.json').write_text(json.dumps(self.manifest))

    def commit(self, *, email=NOREPLY):
        self.git('add', '--all')
        self.git('-c', 'core.hooksPath=/dev/null', 'commit', '--quiet', '--allow-empty',
                 '-m', 'Synthetic product change',
                 env=dict(self.env, GIT_AUTHOR_EMAIL=email, GIT_COMMITTER_EMAIL=email))

    def invoke(self, artifacts=False, *, repo=None):
        args = [sys.executable, '-B', str(CHECK), '--repo', str(repo or self.repo)]
        if artifacts:
            args += ['--artifacts', str(self.artifacts)]
        return subprocess.run(args, capture_output=True, text=True, env=self.env, timeout=20)

    def assert_rejected(self, result, message):
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn(message, result.stderr)

    @staticmethod
    def sums(files):
        return ''.join(hashlib.sha256(data).hexdigest() + '  ' + name + '\n'
                       for name, data in sorted(files.items())).encode()

    @staticmethod
    def archive(path, files, *, root='', uid=0, special=None, duplicate=None):
        with tarfile.open(path, 'w:gz') as tar:
            for name, data in files.items():
                member = tarfile.TarInfo(root + name)
                member.uid = member.gid = uid
                member.uname = member.gname = 'root' if uid == 0 else 'fixture'
                member.mode = 0o644
                member.size = len(data)
                tar.addfile(member, io.BytesIO(data))
            if special:
                member = tarfile.TarInfo(root + special)
                member.type = tarfile.SYMTYPE
                member.linkname = '../../outside'
                member.uname = member.gname = 'root'
                tar.addfile(member)
            if duplicate:
                data = files[duplicate]
                member = tarfile.TarInfo(root + duplicate)
                member.size = len(data)
                member.uname = member.gname = 'root'
                tar.addfile(member, io.BytesIO(data))

    def package(self, *, extra=None, local_path=False, uid=0, special=None,
                duplicate=None, omit_notice=False, altered_source=False, bad_receipt=False):
        revision = self.git('rev-parse', 'HEAD')
        source = {name: (self.repo / name).read_bytes() for name in self.manifest['source']}
        if altered_source:
            source['README.md'] = b'Unexpected altered source\n'
        self.archive(self.artifacts / 'revust-source.tar.gz', source)
        payload = {name: (self.repo / name).read_bytes() for name in self.manifest['payload']}
        payload['bin/revust'] = (('/' + 'Users' + '/fixture/build/cli').encode()
                                 if local_path else b'synthetic executable')
        payload['third_party/cargo/index.json'] = b'[]\n'
        payload['third_party/rust/COPYRIGHT-library.html'] = b'Rust copyright fixture\n'
        payload['third_party/rust/LICENSE-MIT-fixture.txt'] = b'Rust MIT fixture\n'
        payload['third_party/rust/LICENSE-APACHE-fixture.txt'] = b'Rust Apache fixture\n'
        if extra:
            payload.update(extra)
        if omit_notice:
            del payload['third_party/fixture/LICENSE']
        receipt = {'revision': '0' * 40 if bad_receipt else revision, 'version': '0.1.0',
                   'target': 'aarch64-apple-darwin', 'rustc': 'rustc fixture',
                   'cargo': 'cargo fixture',
                   'source_sha256': hashlib.sha256((self.artifacts / 'revust-source.tar.gz').read_bytes()).hexdigest(),
                   'binary_sha256': hashlib.sha256(payload['bin/revust']).hexdigest(),
                   'lockfile_sha256': hashlib.sha256(source['Cargo.lock']).hexdigest()}
        payload['build.json'] = (json.dumps(receipt) + '\n').encode()
        payload['SHA256SUMS'] = self.sums(payload)
        self.archive(self.artifacts / 'revust-0.1.0-aarch64-apple-darwin.tar.gz', payload,
                     root='revust-0.1.0-aarch64-apple-darwin/', uid=uid,
                     special=special, duplicate=duplicate)
        (self.artifacts / 'build.json').write_bytes(payload['build.json'])
        self.outer_sums()

    def outer_sums(self):
        (self.artifacts / 'SHA256SUMS').write_bytes(self.sums({p.name: p.read_bytes()
            for p in self.artifacts.iterdir() if p.name != 'SHA256SUMS'}))

    def test_approved_source_and_complete_package_preserve_dependency_notice_email(self):
        self.package()
        result = self.invoke(artifacts=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_unknown_source_file_is_rejected(self):
        (self.repo / 'internal-notes.md').write_text('Synthetic non-product note')
        self.commit()
        self.assert_rejected(self.invoke(), 'source manifest')

    def test_workflow_file_cannot_be_approved_by_manifest(self):
        (self.repo / 'Plan.md').write_text('Synthetic workflow')
        self.manifest['source'].append('Plan.md')
        self.write_manifest()
        self.commit()
        self.assert_rejected(self.invoke(), 'workflow path')

    def test_historical_private_email_is_rejected_without_printing_it(self):
        email = 'fixture-personal@example.test'
        self.commit(email=email)
        self.commit()
        result = self.invoke()
        self.assert_rejected(result, 'noreply')
        self.assertNotIn(email, result.stdout + result.stderr)

    def test_historical_unapproved_file_is_rejected_after_deletion(self):
        p = self.repo / 'internal-notes.md'
        p.write_text('Synthetic private note')
        self.commit()
        p.unlink()
        self.commit()
        self.assert_rejected(self.invoke(), 'source manifest')

    def test_valid_checksums_do_not_admit_an_extra_package_file(self):
        self.package(extra={'internal-notes.md': b'Synthetic non-product note'})
        self.assert_rejected(self.invoke(artifacts=True), 'payload manifest')

    def test_missing_dependency_notice_is_rejected_even_with_matching_checksums(self):
        self.package(omit_notice=True)
        self.assert_rejected(self.invoke(artifacts=True), 'payload manifest')

    def test_owned_source_local_path_is_rejected_in_utf16(self):
        (self.repo / 'README.md').write_bytes(('/' + 'Users' + '/fixture/work/file').encode('utf-16le'))
        self.commit()
        self.assert_rejected(self.invoke(), 'developer path')

    def test_binary_developer_path_is_rejected(self):
        self.package(local_path=True)
        self.assert_rejected(self.invoke(artifacts=True), 'developer path')

    def test_archive_owner_metadata_is_rejected(self):
        self.package(uid=501)
        self.assert_rejected(self.invoke(artifacts=True), 'archive ownership')

    def test_links_and_traversal_are_rejected(self):
        for path in ['linked-file', '../outside']:
            with self.subTest(path=path):
                self.package(special=path)
                self.assert_rejected(self.invoke(artifacts=True), 'archive')

    def test_duplicate_archive_members_are_rejected(self):
        self.package(duplicate='README.md')
        self.assert_rejected(self.invoke(artifacts=True), 'duplicate')

    def test_outer_checksum_corruption_is_rejected(self):
        self.package()
        with (self.artifacts / 'build.json').open('ab') as stream:
            stream.write(b'changed')
        self.assert_rejected(self.invoke(artifacts=True), 'checksum')

    def test_archived_source_and_receipt_must_match_the_selected_commit(self):
        for options, message in [({'altered_source': True}, 'source bytes'),
                                 ({'bad_receipt': True}, 'receipt')]:
            with self.subTest(options=options):
                self.package(**options)
                self.assert_rejected(self.invoke(artifacts=True), message)

    def test_packager_rejects_unapproved_sources_before_running_cargo(self):
        for name in ['package-local.py', 'check-distribution.py']:
            relative = 'scripts/' + name
            (self.repo / relative).write_bytes((ROOT / relative).read_bytes())
            self.manifest['source'].append(relative)
            self.manifest['payload'].append(relative)
        self.write_manifest()
        self.commit()
        (self.repo / 'internal-notes.md').write_text('Synthetic non-product note')
        self.commit()
        tools = self.root / 'tools'
        tools.mkdir()
        marker = self.root / 'cargo-called'
        for name, body in [('rustc', "printf 'host: aarch64-apple-darwin\\n'\n"),
                           ('cargo', 'touch "' + str(marker) + '"\nexit 97\n')]:
            executable = tools / name
            executable.write_text('#!/bin/sh\n' + body)
            executable.chmod(0o700)
        result = subprocess.run([sys.executable, '-B', str(self.repo / 'scripts/package-local.py'),
                                 '--output', str(self.root / 'output')], cwd=self.repo,
                                env=dict(self.env, PATH=str(tools) + ':/usr/bin:/bin'),
                                capture_output=True, text=True, timeout=20)
        self.assertFalse(marker.exists(), 'packager ran Cargo before rejecting unapproved source')
        self.assert_rejected(result, 'source manifest')

    def test_shallow_history_cannot_claim_metadata_acceptance(self):
        self.commit()
        shallow = self.root / 'shallow'
        subprocess.run(['git', 'clone', '--quiet', '--depth=1', self.repo.as_uri(), str(shallow)],
                       check=True, env=self.env)
        self.assert_rejected(self.invoke(repo=shallow), 'full Git history')


if __name__ == '__main__':
    unittest.main()
