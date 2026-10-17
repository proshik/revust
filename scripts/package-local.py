#!/usr/bin/env python3
"""Build local native packages from a committed snapshot; never install or publish."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import shlex
import subprocess
import tarfile
import tempfile


def run(args, **options):
    return subprocess.check_output(args, text=True, **options).strip()


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True, help='New output directory')
    parser.add_argument('--revision', default='HEAD', help='Committed revision to build')
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]
    revision = run(['git', '-C', str(repo), 'rev-parse', '--verify', '--end-of-options', args.revision + '^{commit}'])
    host = next(line[6:] for line in run(['rustc', '-vV']).splitlines() if line.startswith('host: '))
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    source_archive = output / 'revust-source.tar.gz'
    subprocess.run(['git', '-C', str(repo), 'archive', '--format=tar.gz',
                    '--output=' + str(source_archive), revision], check=True)
    with tempfile.TemporaryDirectory(prefix='revust-package-') as temporary:
        source = Path(temporary) / 'source'
        source.mkdir()
        # The archive is produced above from our own Git objects, not an input tarball.
        subprocess.run(['tar', '-xf', str(source_archive), '-C', str(source)], check=True)
        environment = dict(os.environ, CARGO_TARGET_DIR=str(Path(temporary) / 'target'))
        # Rust diagnostics can embed compile-time paths. Keep developer homes and
        # temporary build directories out of distributed executables.
        flags = (os.environ['CARGO_ENCODED_RUSTFLAGS'].split('\x1f')
                 if 'CARGO_ENCODED_RUSTFLAGS' in os.environ
                 else shlex.split(os.environ.get('RUSTFLAGS', '')))
        flags.extend(['--remap-path-prefix=' + str(Path.home()) + '=/build/user',
                      '--remap-path-prefix=' + temporary + '=/build/package',
                      '--remap-path-prefix=' + str(source) + '=/build/revust'])
        environment['CARGO_ENCODED_RUSTFLAGS'] = '\x1f'.join(flags)
        with (output / 'build.log').open('w') as log:
            subprocess.run(['cargo', 'build', '--release', '--offline', '--locked',
                            '--target', host, '-p', 'revust'],
                           cwd=source, env=environment, stdout=log, stderr=subprocess.STDOUT, check=True)
        metadata = json.loads(run(['cargo', 'metadata', '--offline', '--locked',
                                   '--format-version', '1', '--filter-platform', host],
                                  cwd=source, env=environment))
        version = next(p['version'] for p in metadata['packages'] if p['name'] == 'revust')
        name = 'revust-' + version + '-' + host
        package = Path(temporary) / name
        (package / 'bin').mkdir(parents=True)
        binary = package / 'bin/revust'
        shutil.copy2(Path(environment['CARGO_TARGET_DIR']) / host / 'release/revust', binary)
        # Product files are an explicit distribution manifest. Repository-local
        # workflow files are never copied through a whole-tree documentation walk.
        for item in ('README.md', 'LICENSE', 'Cargo.lock', 'scripts', 'third_party', 'integrations'):
            path = source / item
            if path.is_dir():
                shutil.copytree(path, package / item)
            else:
                shutil.copy2(path, package / item)
        for item in ('CHANGELOG.md', 'CONTRIBUTING.md', 'SECURITY.md',
                     'tests/compat/README.md', 'docs/acceptance.md',
                     'docs/archives.md', 'docs/attribution.md', 'docs/configuration.md',
                     'docs/design.md', 'docs/executor-protocols.json',
                     'docs/executor-protocols.md', 'docs/extensions-v1.md',
                     'docs/hosts.md', 'docs/installation.md', 'docs/local-packaging.md',
                     'docs/releases.md', 'docs/usage.md',
                     'docs/requirements/review-configuration/spec.md',
                     'docs/requirements/standalone-review-runtime/spec.md'):
            path = source / item
            if path.is_dir():
                shutil.copytree(path, package / item)
            elif path.is_file():
                (package / item).parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(path, package / item)
        used = {node['id'] for node in metadata['resolve']['nodes']}
        dependencies = []
        notices = package / 'third_party/cargo'
        notices.mkdir()
        for dependency in metadata['packages']:
            if dependency['id'] not in used or not dependency['source']:
                continue
            directory = Path(dependency['manifest_path']).parent
            files = sorted(p for p in directory.iterdir() if p.is_file() and
                           p.name.upper().startswith(('LICENSE', 'LICENCE', 'COPYING', 'NOTICE', 'COPYRIGHT')))
            if not files:
                raise RuntimeError('Missing dependency notice: ' + dependency['name'])
            destination = notices / (dependency['name'] + '-' + dependency['version'])
            destination.mkdir()
            for file in files:
                shutil.copy2(file, destination / file.name)
            dependencies.append({key: dependency[key] for key in ('name', 'version', 'license', 'source')})
        (notices / 'index.json').write_text(json.dumps(dependencies, indent=2) + '\n')
        rust_docs = Path(run(['rustc', '--print', 'sysroot'])) / 'share/doc/rust'
        standard_notices = [rust_docs / 'COPYRIGHT-library.html']
        for license_name in ('LICENSE-MIT', 'LICENSE-APACHE'):
            matches = sorted((rust_docs / 'html/static.files').glob(license_name + '-*.txt'))
            if not matches:
                raise RuntimeError('Missing Rust licence notice: ' + license_name)
            standard_notices.append(matches[0])
        standard = package / 'third_party/rust'
        standard.mkdir()
        for file in standard_notices:
            shutil.copy2(file, standard / file.name)
        receipt = {'revision': revision, 'target': host, 'version': version,
                   'rustc': run(['rustc', '--version']), 'cargo': run(['cargo', '--version']),
                   'source_sha256': digest(source_archive), 'binary_sha256': digest(binary),
                   'lockfile_sha256': digest(source / 'Cargo.lock')}
        (package / 'build.json').write_text(json.dumps(receipt, indent=2) + '\n')
        with (package / 'SHA256SUMS').open('w') as sums:
            for file in sorted(p for p in package.rglob('*') if p.is_file() and p.name != 'SHA256SUMS'):
                sums.write(digest(file) + '  ' + file.relative_to(package).as_posix() + '\n')
        artifact = output / (name + '.tar.gz')
        with tarfile.open(artifact, 'w:gz') as archive:
            archive.add(package, arcname=name)
        (output / 'build.json').write_text(json.dumps(receipt, indent=2) + '\n')
        (output / 'SHA256SUMS').write_text(''.join(digest(file) + '  ' + file.name + '\n'
                                                for file in (source_archive, artifact, output / 'build.json')))
    print(output)


if __name__ == '__main__':
    main()
