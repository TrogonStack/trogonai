"""Use complete source history without materializing unrelated coverage reports."""
import json
import os
from pathlib import Path
import subprocess
import stat
import sys

from publish import context, git
from sources import GitObjects, relative_path

OWNER = 'coverage-storage-owner.json'


def path_patterns(paths):
    return ['/' + ''.join('\\' + c if c in '*?[] #!' else c for c in relative_path(path))
            for path in dict.fromkeys(['README.md', 'data/.gitkeep', *paths])]


def workspace():
    root = Path(os.environ['GITHUB_WORKSPACE']).resolve(strict=True)
    storage = relative_path(os.environ['COVERAGE_STORAGE_BRANCH'])
    directory = root / storage
    if directory.resolve() != directory:
        raise RuntimeError('Coverage storage path must not traverse a symlink')
    return root, directory


def binding(root, directory):
    return {'workspace': str(root), 'directory': str(directory), 'source': os.environ['GITHUB_SHA'],
            'run': os.environ['GITHUB_RUN_ID'], 'attempt': os.environ['GITHUB_RUN_ATTEMPT']}


def snapshot(stage, root, directory, paths):
    try:
        filesystem = os.statvfs(root)
        print(f'Coverage storage {stage}: available_bytes={filesystem.f_bavail * filesystem.f_frsize} '
              f'available_inodes={filesystem.f_favail}', flush=True)
    except (AttributeError, OSError):
        print(f'Coverage storage {stage}: filesystem capacity unavailable', flush=True)
    if stage == 'after':
        try:
            sizes = []
            for path in dict.fromkeys(['README.md', 'data/.gitkeep', *paths]):
                try:
                    entry = (directory / path).lstat()
                    if stat.S_ISREG(entry.st_mode):
                        sizes.append(entry.st_size)
                except FileNotFoundError:
                    continue
            print(f'Coverage storage after: selected_files={len(sizes)} selected_bytes={sum(sizes)}', flush=True)
        except OSError:
            print('Coverage storage after: selected file sizes unavailable', flush=True)


def prepare():
    storage, _, paths, _ = context()
    patterns = path_patterns(paths)
    root, directory = workspace()
    if directory.exists() or directory.is_symlink():
        raise RuntimeError('Coverage storage path already exists')
    GitObjects(root, os.environ['GITHUB_SHA'])
    if git('-C', str(root), 'rev-parse', '--is-shallow-repository').stdout.strip() != 'false':
        raise RuntimeError('Coverage source requires complete Git history')
    if git('-C', str(root), 'rev-parse', 'HEAD').stdout.strip() != os.environ['GITHUB_SHA']:
        raise RuntimeError('Coverage source checkout changed')
    snapshot('before', root, directory, paths)
    git('-C', str(root), 'fetch', '--no-tags', '--no-filter', 'origin',
        f'+refs/heads/{storage}:refs/remotes/origin/{storage}')
    storage_commit = git('-C', str(root), 'rev-parse', f'refs/remotes/origin/{storage}').stdout.strip()
    print(f'Coverage storage ref: {storage_commit}', flush=True)
    git('-C', str(root), 'worktree', 'add', '--no-checkout', '--detach', str(directory),
        f'refs/remotes/origin/{storage}')
    receipt = Path(git('-C', str(directory), 'rev-parse', '--git-path', OWNER).stdout.strip())
    receipt.write_text(json.dumps(binding(root, directory)) + '\n')
    environment = os.environ.copy()
    environment.pop('COVERAGE_TOKEN', None)
    result = subprocess.run(['git', '-C', str(directory), 'sparse-checkout', 'set', '--no-cone', '--stdin'],
                            input='\n'.join(patterns) + '\n', text=True, capture_output=True,
                            env=environment)
    if result.returncode:
        raise RuntimeError('Coverage storage sparse selection failed')
    git('-C', str(directory), 'checkout', '--quiet', '--detach')
    snapshot('after', root, directory, paths)


def cleanup():
    root, directory = workspace()
    if directory.is_symlink():
        raise RuntimeError('Coverage storage path is a symlink')
    common = Path(git('-C', str(root), 'rev-parse', '--path-format=absolute', '--git-common-dir').stdout.strip())
    registrations = common / 'worktrees'
    for registration in registrations.iterdir() if registrations.exists() else ():
        pointer = registration / 'gitdir'
        if pointer.is_file() and pointer.read_text().strip() == str(directory / '.git'):
            receipt = registration / OWNER
            if not receipt.is_file() or json.loads(receipt.read_text()) != binding(root, directory):
                raise RuntimeError('Coverage storage worktree is not owned by this run')
            git('-C', str(root), 'worktree', 'remove', '--force', str(directory))
            return
    if directory.exists():
        raise RuntimeError('Coverage storage path is not an owned worktree')


def main():
    operation = sys.argv[1:]
    if operation == ['prepare']:
        if 'COVERAGE_TOKEN' in os.environ and not os.environ['COVERAGE_TOKEN']:
            raise RuntimeError('Coverage storage token is required')
        prepare()
    elif operation == ['fetch']:
        storage, _, _, _ = context()
        git('fetch', '--quiet', 'origin', storage)
    elif operation == ['cleanup']:
        cleanup()
    else:
        raise ValueError('Expected prepare, fetch or cleanup')


if __name__ == '__main__':
    try:
        main()
    except (KeyError, OSError, ValueError, RuntimeError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
