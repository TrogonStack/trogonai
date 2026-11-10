#!/usr/bin/env python3
"""Publish coverage commits without replacing a newer destination or baseline."""
import base64
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import sys
from urllib.parse import urlsplit

from sources import repository_name

MAX_ATTEMPTS = 8


def git_environment(directory=None):
    environment = os.environ.copy()
    if 'COVERAGE_TOKEN' in environment:
        token = environment.pop('COVERAGE_TOKEN')
        if not token:
            raise RuntimeError('Coverage storage token is required')
        server = environment.get('GITHUB_SERVER_URL', 'https://github.com').rstrip('/')
        url = urlsplit(server)
        if url.scheme != 'https' or not url.hostname or url.username or url.password or url.path or url.query or url.fragment:
            raise RuntimeError('Invalid coverage storage server')
        repository = repository_name(environment['GITHUB_REPOSITORY'])
        routes = []
        for arguments in [('remote', 'get-url', '--all', 'origin'), ('remote', 'get-url', '--push', '--all', 'origin')]:
            route = subprocess.run(['git', *arguments], cwd=directory, env=environment,
                                   text=True, capture_output=True)
            urls = route.stdout.splitlines()
            if route.returncode or len(urls) != 1:
                raise RuntimeError('Invalid coverage storage remote')
            target = urlsplit(urls[0])
            if (target.scheme != url.scheme or target.netloc.casefold() != url.netloc.casefold()
                    or target.username or target.password or target.query or target.fragment
                    or target.path.rstrip('/').casefold() not in ('/' + repository, '/' + repository + '.git')):
                raise RuntimeError('Coverage storage remote does not match the trusted repository')
            routes.append(urls[0])
        authorization = base64.b64encode(('x-access-token:' + token).encode()).decode()
        settings = [('http.extraheader', ''), ('http.' + server + '/.extraheader', '')]
        for route in dict.fromkeys(routes):
            header = 'http.' + route + '.extraheader'
            settings.extend([(header, ''), (header, 'AUTHORIZATION: basic ' + authorization)])
        settings.extend([('credential.helper', ''), ('core.askPass', '')])
        count = int(environment.get('GIT_CONFIG_COUNT', '0'))
        if count < 0:
            raise RuntimeError('Invalid inherited Git configuration')
        for key, value in settings:
            environment[f'GIT_CONFIG_KEY_{count}'] = key
            environment[f'GIT_CONFIG_VALUE_{count}'] = value
            count += 1
        environment.update(GIT_CONFIG_COUNT=str(count), GIT_TERMINAL_PROMPT='0', GCM_INTERACTIVE='Never',
                           GIT_ASKPASS='', SSH_ASKPASS='')
    return environment


def git(*arguments, check=True):
    operation = arguments[2] if arguments[0] == '-C' else arguments[0]
    environment = os.environ.copy()
    environment.pop('COVERAGE_TOKEN', None)
    if operation in ('fetch', 'push'):
        environment = git_environment(arguments[1] if arguments[0] == '-C' else None)
    result = subprocess.run(['git', *arguments], text=True, capture_output=True, env=environment)
    if check and result.returncode:
        raise RuntimeError(f'git {arguments[0]} failed with exit {result.returncode}')
    return result


def relative_path(value):
    path = PurePosixPath(value)
    if path.is_absolute() or '..' in path.parts or '\\' in value:
        raise RuntimeError('coverage storage path must remain relative')
    return str(path)


def identity(revision, path):
    result = git('rev-parse', '--verify', f'{revision}:{path}', check=False)
    if result.returncode == 0:
        return result.stdout.strip()
    tree = git('ls-tree', '-z', revision, '--', path)
    if tree.stdout:
        raise RuntimeError('coverage destination is not an ordinary blob')
    return None


def context():
    storage = os.environ['COVERAGE_STORAGE_BRANCH']
    if git('check-ref-format', f'refs/heads/{storage}', check=False).returncode:
        raise RuntimeError('invalid coverage storage branch')
    branch = os.environ.get('GITHUB_HEAD_REF') or os.environ['GITHUB_REF'].removeprefix('refs/heads/')
    directory = relative_path(f"data/{branch}/{os.environ['COVERAGE_STORAGE_SUBDIRECTORY']}")
    baseline = relative_path(f"data/{os.environ['COVERAGE_DIFF_BRANCH']}/{os.environ['COVERAGE_STORAGE_SUBDIRECTORY']}/coverage.xml")
    baseline_sources = str(PurePosixPath(baseline).with_name('sources.json'))
    paths = [f'{directory}/coverage.xml', f'{directory}/badge.svg', f'{directory}/sources.json']
    if os.environ['COVERAGE_DIFF_ENABLED'] == 'true':
        paths.extend([baseline, baseline_sources])
    baseline_writer = (os.environ.get('GITHUB_EVENT_NAME') == 'push'
        and not os.environ.get('GITHUB_HEAD_REF')
        and os.environ.get('GITHUB_REF') == 'refs/heads/' + os.environ['COVERAGE_DIFF_BRANCH'])
    if baseline in paths[:3] and not baseline_writer:
        raise RuntimeError('coverage report cannot overwrite the comparison baseline')
    return storage, baseline, list(dict.fromkeys(paths)), baseline_writer


def main():
    storage, baseline, paths, baseline_writer = context()
    baseline_sources = str(PurePosixPath(baseline).with_name('sources.json'))
    receipt = Path(git('rev-parse', '--git-path', 'coverage-publication-receipt.json').stdout.strip())
    binding = {'storage': storage, 'paths': paths, 'source': os.environ['GITHUB_SHA'], 'baseline_writer': baseline_writer}
    if sys.argv[1:] == ['init']:
        expected = {path: identity('HEAD', path) for path in paths}
        if baseline in paths and any(expected[path] is None for path in (baseline, baseline_sources)):
            raise RuntimeError('coverage comparison baseline is missing')
        receipt.write_text(json.dumps({'binding': binding, 'expected': expected}) + '\n')
        return
    if sys.argv[1:] != ['push']:
        raise RuntimeError('expected init or push')
    state = json.loads(receipt.read_text())
    if state['binding'] != binding:
        raise RuntimeError('coverage publication receipt belongs to another report')
    if not baseline_writer and baseline in paths and any(identity('HEAD', path) != state['expected'][path] for path in (baseline, baseline_sources)):
        raise RuntimeError('local coverage comparison baseline changed')
    remote_ref = 'refs/remotes/origin/coverage-publication'
    for attempt in range(1, MAX_ATTEMPTS + 1):
        git('fetch', '--quiet', 'origin', f'+refs/heads/{storage}:{remote_ref}')
        for path, expected in state['expected'].items():
            if identity(remote_ref, path) != expected:
                raise RuntimeError(f'coverage destination changed after comparison: {path}')
        rebased = git('rebase', remote_ref, check=False)
        if rebased.returncode:
            git('rebase', '--abort', check=False)
            raise RuntimeError('coverage publication conflicts with newer storage content')
        pushed = git('push', '--quiet', 'origin', f'HEAD:refs/heads/{storage}', check=False)
        if pushed.returncode == 0:
            state['expected'] = {path: identity('HEAD', path) for path in paths}
            receipt.write_text(json.dumps(state) + '\n')
            print(f'Coverage publication accepted on attempt {attempt}')
            return
    raise RuntimeError(f'coverage publication contention exceeded {MAX_ATTEMPTS} attempts')


if __name__ == '__main__':
    try:
        main()
    except (KeyError, OSError, ValueError, RuntimeError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
