#!/usr/bin/env python3
"""Validate approved product sources, public commit metadata and release archives."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tarfile


class Rejected(ValueError):
    pass


NOREPLY = re.compile(rb'(?:[0-9]+\+)?[A-Za-z0-9][A-Za-z0-9_.\-\[\]]*@users\.noreply\.github\.com')
DEVELOPER_PATH = re.compile(
    r"(?<![A-Za-z0-9_$])/(?:Users|home)/[^/\s\x00\"'<>]+/|/(?:private/)?var/folders/|/(?:private/)?tmp/revust-package-")
WORKFLOW_NAMES = {'AGENTS.md', 'AGENTS.override.md', 'CLAUDE.md', 'Plan.md'}
WORKFLOW_DIRS = {'.git', '.guild', '.local', '.codex', '.agents', '.claude', '.revust'}
NOTICE_PREFIXES = ('LICENSE', 'LICENCE', 'COPYING', 'NOTICE', 'COPYRIGHT')
MAX_FILE = 16 * 1024 * 1024
MAX_TOTAL = 128 * 1024 * 1024


def require(condition, reason):
    if not condition:
        raise Rejected(reason)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def product_path(name):
    require(isinstance(name, str) and name and not any(c in name for c in '\x00\r\n\\'),
            'invalid product path')
    path = PurePosixPath(name)
    require(not path.is_absolute() and str(path) == name and
            not any(p in {'', '.', '..'} for p in path.parts), 'unsafe archive or product path')
    require(not any(p in WORKFLOW_DIRS for p in path.parts) and
            path.name not in WORKFLOW_NAMES and not path.name.startswith('.env'),
            'workflow path is not a product file')
    return name


def check_content(data):
    texts = [data.decode('utf-8', errors='ignore')]
    if b'\x00' in data:
        texts += [data.decode(encoding, errors='ignore') for encoding in ('utf-16le', 'utf-16be')]
    require(not any(DEVELOPER_PATH.search(text) for text in texts),
            'developer path found in distributed content')


def read_json(data, reason):
    def unique(items):
        result = {}
        for key, value in items:
            require(key not in result, reason + ': duplicate field')
            result[key] = value
        return result
    try:
        return json.loads(data, object_pairs_hook=unique)
    except (ValueError, UnicodeError):
        raise Rejected(reason) from None


def manifest(data):
    value = read_json(data, 'invalid distribution manifest')
    require(isinstance(value, dict) and set(value) == {'version', 'source', 'payload'}
            and type(value['version']) is int and value['version'] == 1,
            'unsupported distribution manifest')
    for kind in ('source', 'payload'):
        names = value[kind]
        require(isinstance(names, list) and names and all(isinstance(p, str) for p in names),
                'invalid ' + kind + ' manifest')
        require(len(names) == len(set(names)), 'duplicate ' + kind + ' manifest path')
        for name in names:
            product_path(name)
        value[kind] = set(names)
    require({'distribution.json', 'Cargo.lock', 'LICENSE'} <= value['source'] and
            {'distribution.json', 'Cargo.lock', 'LICENSE'} <= value['payload'] <= value['source'],
            'payload manifest must select approved source files and notices')
    return value


class Sources:
    def __init__(self, repo, revision):
        self.repo = repo
        self.cache = {}
        require(self.git('rev-parse', '--is-shallow-repository').strip() == b'false',
                'full Git history is required for distribution validation')
        self.revision = self.git('rev-parse', '--verify', '--end-of-options',
                                 revision + '^{commit}').decode().strip()
        self.tree = self.read_tree(self.revision)
        require('distribution.json' in self.tree, 'source manifest is missing')
        self.policy = manifest(self.blob(self.tree['distribution.json']))
        for commit in self.git('rev-list', self.revision).decode().splitlines():
            self.check_commit(commit)

    def git(self, *args):
        try:
            return subprocess.check_output(['git', '-C', str(self.repo), *args], stderr=subprocess.PIPE)
        except subprocess.CalledProcessError:
            raise Rejected('cannot inspect selected Git revision') from None

    def blob(self, oid):
        if oid not in self.cache:
            size = self.git('cat-file', '-s', oid).strip()
            require(size.isdigit() and int(size) <= MAX_FILE, 'source file exceeds validation limit')
            self.cache[oid] = self.git('cat-file', 'blob', oid)
            check_content(self.cache[oid])
        return self.cache[oid]

    def read_tree(self, revision):
        tree = {}
        for entry in self.git('ls-tree', '-r', '-z', '--full-tree', revision).split(b'\x00'):
            if not entry:
                continue
            header, path = entry.split(b'\t', 1)
            mode, kind, oid = header.split()
            require(mode in (b'100644', b'100755') and kind == b'blob',
                    'source archive must contain regular files only')
            name = product_path(path.decode('utf-8'))
            require(name not in tree, 'duplicate source path')
            tree[name] = oid.decode()
        return tree

    def check_commit(self, commit):
        data = self.git('cat-file', 'commit', commit)
        identities = set()
        for line in data.split(b'\n\n', 1)[0].splitlines():
            for role in (b'author', b'committer'):
                if line.startswith(role + b' '):
                    match = re.fullmatch(rb'.+ <([^<>\n]+)> \d+ [+-]\d{4}', line[len(role) + 1:])
                    require(match and (NOREPLY.fullmatch(match[1]) or match[1] == b'noreply@github.com'),
                            'commit author and committer must use GitHub noreply addresses')
                    identities.add(role)
        require(len(identities) == 2, 'commit author and committer metadata is missing')
        check_content(data)
        tree = self.read_tree(commit)
        if 'distribution.json' in tree:
            policy = manifest(self.blob(tree['distribution.json']))
            require(set(tree) == policy['source'], 'source manifest does not match the committed files')
        else:
            # Commits predating the policy still have to fit the reviewed product list.
            require(set(tree) <= self.policy['source'], 'historical source manifest contains unapproved files')
        for oid in tree.values():
            self.blob(oid)

    def files(self):
        return {name: self.blob(oid) for name, oid in self.tree.items()}


def checksums(data):
    result = {}
    try:
        for line in data.decode('utf-8').splitlines():
            require(re.fullmatch(r'[0-9a-f]{64}  .+', line), 'invalid checksum record')
            sha, name = line.split('  ', 1)
            product_path(name)
            require(name not in result, 'duplicate checksum record')
            result[name] = sha
    except UnicodeError:
        raise Rejected('invalid checksum encoding') from None
    require(result, 'empty checksum list')
    return result


def bounded_file(path):
    require(path.is_file() and not path.is_symlink() and path.stat().st_size <= MAX_TOTAL,
            'invalid or oversized release artifact')
    return path.read_bytes()


def read_tar(path, prefix, revision):
    files, directories, seen = {}, set(), set()
    total = 0
    with tarfile.open(path, 'r|gz') as archive:
        for member in archive:
            require(len(seen) < 10000, 'archive has too many members')
            name = member.name.rstrip('/') if member.isdir() else member.name
            require(name not in seen, 'duplicate archive member')
            seen.add(name)
            product_path(name)
            require(member.isfile() or member.isdir(), 'archive links and special files are forbidden')
            require((member.uid, member.gid, member.uname, member.gname) == (0, 0, 'root', 'root'),
                    'archive ownership must use portable root metadata')
            require(not member.mode & 0o7000, 'archive special permission bits are forbidden')
            require(set(member.pax_headers) <= {'path', 'size', 'mtime', 'comment'},
                    'unsupported archive metadata')
            if 'comment' in member.pax_headers:
                require(member.pax_headers['comment'] == revision, 'archive comment must identify the source commit')
            for key, value in member.pax_headers.items():
                if key in {'size', 'mtime'}:
                    require(re.fullmatch(r'[0-9]+(?:\.[0-9]+)?', value), 'invalid archive numeric metadata')
                check_content(value.encode())
            if prefix:
                if name == prefix.rstrip('/'):
                    require(member.isdir(), 'invalid archive root')
                    continue
                require(name.startswith(prefix), 'unexpected archive root')
                name = name[len(prefix):]
                product_path(name)
            if member.isdir():
                directories.add(name)
                continue
            require(0 <= member.size <= MAX_FILE and total + member.size <= MAX_TOTAL,
                    'archive exceeds validation limits')
            total += member.size
            data = archive.extractfile(member).read()
            require(len(data) == member.size, 'truncated archive file')
            check_content(data)
            files[name] = data
    parents = {str(parent) for name in files for parent in PurePosixPath(name).parents if str(parent) != '.'}
    require(directories <= parents, 'unexpected empty archive directory')
    return files


def generated_files(payload):
    expected = {'bin/revust', 'build.json', 'SHA256SUMS', 'third_party/cargo/index.json',
                'third_party/rust/COPYRIGHT-library.html'}
    index = read_json(payload.get('third_party/cargo/index.json', b''), 'invalid dependency notice index')
    require(isinstance(index, list), 'invalid dependency notice index')
    dependency_dirs = set()
    for item in index:
        require(isinstance(item, dict) and set(item) == {'name', 'version', 'license', 'source'},
                'invalid dependency notice identity')
        require(isinstance(item['name'], str) and re.fullmatch(r'[A-Za-z0-9_-]+', item['name']) and
                isinstance(item['version'], str) and re.fullmatch(r'[0-9][A-Za-z0-9.+_-]*', item['version']),
                'invalid dependency notice identity')
        directory = 'third_party/cargo/' + item['name'] + '-' + item['version'] + '/'
        require(directory not in dependency_dirs, 'duplicate dependency notice identity')
        dependency_dirs.add(directory)
        notices = {name for name in payload if name.startswith(directory) and
                   '/' not in name[len(directory):] and name[len(directory):].upper().startswith(NOTICE_PREFIXES)}
        require(notices, 'missing dependency notice')
        expected |= notices
    for kind in ('MIT', 'APACHE'):
        notices = {name for name in payload if re.fullmatch(
            r'third_party/rust/LICENSE-' + kind + r'-[A-Za-z0-9_.-]+\.txt', name)}
        require(notices, 'missing Rust licence notice')
        expected |= notices
    return expected


def validate_artifacts(sources, directory):
    sums = checksums(bounded_file(directory / 'SHA256SUMS'))
    require(len(sums) == 3, 'release checksums must identify exactly three artifacts')
    for name, expected in sums.items():
        require('/' not in name and digest(bounded_file(directory / name)) == expected,
                'release checksum mismatch')
    receipt_bytes = bounded_file(directory / 'build.json')
    receipt = read_json(receipt_bytes, 'invalid build receipt')
    fields = {'revision', 'target', 'version', 'rustc', 'cargo', 'source_sha256',
              'binary_sha256', 'lockfile_sha256'}
    require(isinstance(receipt, dict) and set(receipt) == fields and
            all(isinstance(v, str) and v for v in receipt.values()), 'invalid build receipt')
    require(receipt['revision'] == sources.revision and
            re.fullmatch(r'[0-9][A-Za-z0-9.+_-]*', receipt['version']) and
            re.fullmatch(r'[A-Za-z0-9_]+(?:-[A-Za-z0-9_]+)+', receipt['target']),
            'build receipt does not identify the selected source')
    check_content(receipt_bytes)
    package_name = 'revust-' + receipt['version'] + '-' + receipt['target']
    expected_outer = {'revust-source.tar.gz', package_name + '.tar.gz', 'build.json'}
    require(set(sums) == expected_outer, 'unexpected release artifact names')
    require({p.name for p in directory.iterdir()} <= expected_outer | {'SHA256SUMS', 'build.log'},
            'unexpected release artifact file')
    require(digest(bounded_file(directory / 'revust-source.tar.gz')) == receipt['source_sha256'],
            'source archive receipt checksum mismatch')
    source_files = read_tar(directory / 'revust-source.tar.gz', '', sources.revision)
    require(source_files == sources.files(), 'archived source bytes differ from the selected commit')
    payload = read_tar(directory / (package_name + '.tar.gz'), package_name + '/', sources.revision)
    expected = sources.policy['payload'] | generated_files(payload)
    require(set(payload) == expected, 'payload manifest does not match packaged files and notices')
    inner = checksums(payload['SHA256SUMS'])
    require(set(inner) == set(payload) - {'SHA256SUMS'} and
            all(digest(payload[name]) == expected for name, expected in inner.items()),
            'payload checksum mismatch')
    require(payload['build.json'] == receipt_bytes and
            digest(payload['bin/revust']) == receipt['binary_sha256'] and
            digest(source_files['Cargo.lock']) == receipt['lockfile_sha256'],
            'payload build receipt mismatch')
    require(all(payload[name] == source_files[name] for name in sources.policy['payload']),
            'payload source bytes differ from the selected commit')
    return len(payload)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument('--revision', default='HEAD')
    parser.add_argument('--artifacts', type=Path)
    args = parser.parse_args()
    try:
        sources = Sources(args.repo.resolve(), args.revision)
        count = validate_artifacts(sources, args.artifacts.resolve()) if args.artifacts else None
        print('Distribution source/history checks passed' +
              ('; %d packaged files verified' % count if count is not None else ''))
        return 0
    except (Rejected, OSError, UnicodeError, tarfile.TarError, KeyError, TypeError) as error:
        # Validation reasons contain no matched data, personal identity or raw subprocess output.
        reason = str(error) if isinstance(error, Rejected) else 'cannot validate distribution input'
        print('Distribution rejected: ' + reason, file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
