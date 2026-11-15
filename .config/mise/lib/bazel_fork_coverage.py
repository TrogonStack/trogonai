"""Bind fork coverage data to GitHub state before comparison or publication."""

from dataclasses import asdict, dataclass
from fractions import Fraction
import hashlib
import html
import io
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
import zipfile

WORKFLOW = '.github/workflows/ci-bazel.yml'
STORAGE = '_xml_coverage_reports_bazel'
MARKER = '<!-- bazel-fork-coverage -->'
MAX_REPORT = 128 * 1024 * 1024
MAX_ARCHIVE = 16 * 1024 * 1024
MAX_RESULT = 64000
MAX_DETAIL = 4000


def require(condition):
    if not condition:
        raise ValueError('Invalid or stale fork coverage input')


def integer(value):
    require(type(value) is int and 0 < value < 2**53)
    return value


def sha(value):
    require(isinstance(value, str) and re.fullmatch('[0-9a-f]{40}', value) and value != '0' * 40)
    return value


def repository(value):
    require(isinstance(value, dict))
    integer(value['id'])
    require(isinstance(value['full_name'], str) and re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.-]*/[A-Za-z0-9][A-Za-z0-9_.-]*', value['full_name']))
    return value['id'], value['full_name'].lower()


def unique_json(data):
    def pairs(items):
        result = {}
        for key, value in items:
            require(key not in result)
            result[key] = value
        return result
    return json.loads(data, object_pairs_hook=pairs)


class GitHub:
    def __init__(self):
        self.repo = os.environ['GITHUB_REPOSITORY'].lower()
        self.repo_id = integer(int(os.environ['GITHUB_REPOSITORY_ID']))
        require(repository({'id': self.repo_id, 'full_name': self.repo}) == (self.repo_id, self.repo))

    def request(self, endpoint, *, raw=False, method='GET', body=None, limit=8 * 1024 * 1024):
        require(endpoint.startswith('/') and not endpoint.startswith('//'))
        command = ['gh', 'api', '--hostname', 'github.com', '--method', method]
        if raw:
            command += ['--header', 'Accept: application/vnd.github.raw+json']
        if body is not None:
            command += ['--input', '-']
        command += [f'repos/{self.repo}{endpoint}']
        environment = {key: os.environ[key] for key in ('PATH', 'HOME', 'SYSTEMROOT', 'GH_TOKEN') if key in os.environ}
        result = subprocess.run(command, input=None if body is None else json.dumps(body).encode(),
                                env=environment, capture_output=True, timeout=120)
        require(result.returncode == 0 and len(result.stdout) <= limit)
        return result.stdout if raw else unique_json(result.stdout)

    def pages(self, endpoint, field=None):
        rows = []
        total = None
        for page in range(1, 12):
            separator = '&' if '?' in endpoint else '?'
            response = self.request(f'{endpoint}{separator}per_page=100&page={page}')
            if field:
                require(type(response['total_count']) is int and 0 <= response['total_count'] <= 1000)
                if total is None:
                    total = response['total_count']
                require(total == response['total_count'])
                values = response[field]
            else:
                values = response
            require(isinstance(values, list) and len(values) <= 100)
            rows.extend(values)
            require(len(rows) <= 1000)
            if len(values) < 100:
                require(total is None or len(rows) == total)
                ids = [integer(row['id']) for row in rows]
                require(len(ids) == len(set(ids)))
                return rows
        raise ValueError('Incomplete GitHub pagination')


@dataclass(frozen=True)
class Binding:
    repository: str
    repository_id: int
    consumer_run: int
    consumer_attempt: int
    run_id: int
    run_attempt: int
    run_number: int
    workflow_id: int
    pr: int
    pr_id: int
    head_repository: str
    head_repository_id: int
    head_ref: str
    base_ref: str
    head_sha: str
    base_sha: str
    measured_sha: str
    reset: bool


@dataclass(frozen=True)
class Measurement:
    repository: str
    repository_id: int
    run_id: int
    run_attempt: int
    pr: int
    base_ref: str
    head_sha: str
    base_sha: str
    measured_sha: str
    reset: bool


def bind(github):
    require(os.environ['GITHUB_EVENT_NAME'] == 'workflow_run')
    event = unique_json(Path(os.environ['GITHUB_EVENT_PATH']).read_bytes())
    require(event['action'] == 'completed' and repository(event['repository']) == (github.repo_id, github.repo))
    consumer_id = integer(int(os.environ['GITHUB_RUN_ID']))
    consumer_attempt = integer(int(os.environ['GITHUB_RUN_ATTEMPT']))
    consumer = github.request(f'/actions/runs/{consumer_id}')
    require(consumer['id'] == consumer_id and consumer['run_attempt'] == consumer_attempt)
    require(repository(consumer['repository']) == (github.repo_id, github.repo))
    require(consumer['event'] == 'workflow_run' and consumer['path'] == '.github/workflows/ci-bazel-fork-coverage.yml' and consumer['status'] == 'in_progress')
    event_run = event['workflow_run']
    run_id, attempt = integer(event_run['id']), integer(event_run['run_attempt'])
    workflow = github.request('/actions/workflows/ci-bazel.yml')
    require(workflow['path'] == WORKFLOW and workflow['name'] == 'CI Bazel')
    workflow_id = integer(workflow['id'])
    run = github.request(f'/actions/runs/{run_id}/attempts/{attempt}')
    latest = github.request(f'/actions/runs/{run_id}')
    fields = ('id', 'run_attempt', 'run_number', 'workflow_id', 'path', 'event', 'status', 'conclusion', 'head_sha', 'head_branch')
    require(all(run[key] == latest[key] == event_run[key] for key in fields))
    require(run['id'] == run_id and run['run_attempt'] == attempt and run['workflow_id'] == workflow_id)
    require(run['path'] == WORKFLOW and run['event'] == 'pull_request' and run['status'] == 'completed' and run['conclusion'] in ('success', 'failure'))
    jobs = github.pages(f'/actions/runs/{run_id}/attempts/{attempt}/jobs', 'jobs')
    verify = [job for job in jobs if job['name'] == 'verify']
    require(len(verify) == 1 and verify[0]['status'] == 'completed' and verify[0]['conclusion'] == 'success')
    require(all(repository(item['repository']) == (github.repo_id, github.repo) for item in (run, latest, event_run)))
    head_id, head_repo = repository(run['head_repository'])
    require(head_id != github.repo_id)
    require(all(repository(item['head_repository']) == (head_id, head_repo) for item in (latest, event_run)))
    head_sha = sha(run['head_sha'])
    require(isinstance(run['head_branch'], str) and 0 < len(run['head_branch']) <= 1024)

    # workflow_run.pull_requests may be empty for forks. GitHub's commit-to-PR
    # association supplies the destination; the artifact never supplies it.
    associated = github.pages(f'/commits/{head_sha}/pulls')
    candidates = [pr for pr in associated if pr['state'] == 'open'
                  and pr['head']['sha'] == head_sha and repository(pr['head']['repo']) == (head_id, head_repo)
                  and repository(pr['base']['repo']) == (github.repo_id, github.repo)]
    require(len(candidates) == 1)
    number = integer(candidates[0]['number'])
    pr = github.request(f'/pulls/{number}')
    require(pr['id'] == candidates[0]['id'] and pr['number'] == number and pr['state'] == 'open' and pr['merged'] is False)
    require(repository(pr['base']['repo']) == (github.repo_id, github.repo))
    require(repository(pr['head']['repo']) == (head_id, head_repo) and pr['head']['sha'] == head_sha and pr['head']['ref'] == run['head_branch'])
    base_sha, measured_sha = sha(pr['base']['sha']), sha(pr['merge_commit_sha'])
    base_ref = pr['base']['ref']
    require(isinstance(base_ref, str) and 0 < len(base_ref) <= 1024)
    for item in (run, latest, event_run):
        pulls = item['pull_requests']
        require(isinstance(pulls, list))
        if pulls:
            require(len(pulls) == 1 and pulls[0]['id'] == pr['id'] and pulls[0]['number'] == number)
            require(pulls[0]['head']['sha'] == head_sha and pulls[0]['base']['sha'] == base_sha)
            require(pulls[0]['base']['ref'] == base_ref)
    commit = github.request(f'/git/commits/{measured_sha}')
    require(commit['sha'] == measured_sha and [row['sha'] for row in commit['parents']] == [base_sha, head_sha])
    runs = github.pages(f'/actions/workflows/{workflow_id}/runs?event=pull_request&head_sha={head_sha}', 'workflow_runs')
    applicable = [row for row in runs if repository(row['head_repository']) == (head_id, head_repo) and row['head_branch'] == run['head_branch']]
    require(applicable and all(row['workflow_id'] == workflow_id and row['head_sha'] == head_sha and row['event'] == 'pull_request' for row in applicable))
    newest = max(applicable, key=lambda row: (integer(row['run_number']), integer(row['run_attempt'])))
    require(newest['id'] == run_id and newest['run_attempt'] == attempt)
    require(isinstance(pr['labels'], list) and all(isinstance(row['name'], str) for row in pr['labels']))
    return Binding(github.repo, github.repo_id, consumer_id, consumer_attempt,
                   run_id, attempt, integer(run['run_number']), workflow_id, number, integer(pr['id']), head_repo, head_id,
                   run['head_branch'], base_ref, head_sha, base_sha, measured_sha,
                   any(row['name'] == 'rust:coverage-baseline-reset' for row in pr['labels']))


def report_artifact(github, binding, directory):
    artifacts = github.pages(f'/actions/runs/{binding.run_id}/artifacts', 'artifacts')
    matches = [row for row in artifacts if row['name'] == f'bazel-coverage-{binding.run_attempt}']
    require(len(matches) == 1)
    artifact = matches[0]
    require(artifact['expired'] is False and 0 < integer(artifact['size_in_bytes']) <= MAX_ARCHIVE)
    source = artifact['workflow_run']
    for name in ('id', 'repository_id', 'head_repository_id'):
        integer(source[name])
    require(source == {**source, 'id': binding.run_id, 'repository_id': binding.repository_id,
                       'head_repository_id': binding.head_repository_id, 'head_sha': binding.head_sha, 'head_branch': binding.head_ref})
    data = github.request(f'/actions/artifacts/{integer(artifact["id"])}/zip', raw=True, limit=MAX_ARCHIVE)
    require(artifact['digest'] == 'sha256:' + hashlib.sha256(data).hexdigest())
    expected = {'coverage-bazel.xml', 'coverage-context.json'}
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        entries = archive.infolist()
        require(len(entries) == len(expected) and {row.filename for row in entries} == expected)
        for row in entries:
            require(not row.flag_bits & 1 and row.compress_type in (zipfile.ZIP_DEFLATED, zipfile.ZIP_STORED))
            require(stat.S_IFMT(row.external_attr >> 16) in (0, stat.S_IFREG))
            require(0 < row.file_size <= (MAX_REPORT if row.filename.endswith('.xml') else 4096))
            (directory / row.filename).write_bytes(archive.read(row))
    context = unique_json((directory / 'coverage-context.json').read_bytes())
    require(type(context['version']) is int)
    for name in ('repository_id', 'run_id', 'run_attempt', 'pr'):
        integer(context[name])
    require(context == {'version': 1, 'repository': binding.repository, 'repository_id': binding.repository_id,
                        'run_id': binding.run_id, 'run_attempt': binding.run_attempt, 'pr': binding.pr,
                        'head_sha': binding.head_sha, 'base_sha': binding.base_sha, 'base_ref': binding.base_ref, 'measured_sha': binding.measured_sha})


def policy(current, baseline, status, reset):
    def percentage(counts):
        require(set(counts) == {'statements', 'misses'} and type(counts['statements']) is int and type(counts['misses']) is int)
        require(0 < counts['statements'] < 2**53 and 0 <= counts['misses'] <= counts['statements'])
        return Fraction(100 * (counts['statements'] - counts['misses']), counts['statements'])
    now, previous = percentage(current), percentage(baseline)
    require(type(status) is int and status in (0, 2, 3) and type(reset) is bool)
    return now >= 95 and (reset or (status == 0 and now >= previous))


def compare_reports(github, binding, directory):
    tools = Path(os.environ['COVERAGE_TOOLS']).resolve()
    sys.path.insert(0, str(tools))
    import report
    import sources

    root = Path(os.environ['GITHUB_WORKSPACE']).resolve()
    storage = github.request(f'/git/ref/heads/{STORAGE}')
    require(storage['object']['type'] == 'commit')
    storage_sha = sha(storage['object']['sha'])
    baseline = directory / 'baseline'
    baseline.mkdir()
    for name in ('coverage.xml', 'sources.json'):
        (baseline / name).write_bytes(github.request(f'/contents/data/main/{name}?ref={storage_sha}', raw=True, limit=MAX_REPORT))
    _, manifest = sources.load_manifest(baseline / 'coverage.xml', baseline / 'sources.json', binding.repository)
    # Fetch objects from the base repository only. Never check out measured
    # code, consult its configuration, or run a tool from the artifact.
    for commit in (binding.measured_sha, binding.base_sha):
        try:
            sources.git(root, 'cat-file', '-e', commit + '^{commit}')
        except ValueError:
            sources.git(root, 'fetch', '--no-tags', '--no-recurse-submodules', 'https://github.com/' + binding.repository + '.git', commit)
    sources.git(root, 'merge-base', '--is-ancestor', manifest['commit'], binding.base_sha)
    current = directory / 'current'
    tree = sources.xml_tree(sources.bounded_read(directory / 'coverage-bazel.xml'))
    names = [node.attrib['filename'] for node in tree.findall('.//class')]
    require(names and len(names) == len(set(names)))
    paths = {name: 'rsworkspace/' + sources.relative_path(name) for name in names}
    sources.make_pair(directory / 'coverage-bazel.xml', current / 'sources.json', binding.repository,
                      sources.GitObjects(root, binding.measured_sha), paths)
    os.environ.update(GITHUB_SHA=binding.measured_sha, COVERAGE_BASE_SHA=binding.base_sha)
    status = report.diff(baseline / 'coverage.xml', current / 'coverage.xml', current / 'sources.json', directory / 'diff')
    require(status in (0, 2, 3))
    os.chdir(directory)
    for xml, output in ((current / 'coverage.xml', '.coverage-output'), (baseline / 'coverage.xml', '.coverage-output.diff-branch')):
        result = subprocess.run(['pycobertura', 'show', str(xml), '--output', output], capture_output=True, timeout=120)
        require(result.returncode == 0)
    now = asdict(report.CoverageCounts.read('.coverage-output'))
    previous = asdict(report.CoverageCounts.read('.coverage-output.diff-branch'))
    passed = policy(now, previous, status, binding.reset)
    # Apply the shared helper's exact policy contract as well as its diff.
    os.environ.update(COVERAGE_INPUTS_THRESHOLD='95', COVERAGE_CAPTURE_STATUS='0', COVERAGE_BASELINE_STATUS='0',
                      COVERAGE_ENV_PYCOBERTURA_STATUS=str(status), COVERAGE_INPUTS_PYCOBERTURA_EXCEPTION_FAILURE='true')
    for name in ('UNCOVERED_STATEMENTS_INCREASE_FAILURE', 'NEW_UNCOVERED_STATEMENTS_FAILURE', 'COVERAGE_RATE_REDUCTION_FAILURE'):
        os.environ['COVERAGE_INPUTS_' + name] = str(not binding.reset).lower()
    shared_passed = all(subprocess.run([sys.executable, str(tools / 'report.py'), command], capture_output=True, timeout=120).returncode == 0 for command in ('threshold', 'gate'))
    require(shared_passed == passed)
    detail = sources.bounded_read(directory / 'diff').decode('utf-8')
    detail = detail[:MAX_DETAIL] + ('\n[Diff truncated.]' if len(detail) > MAX_DETAIL else '')
    return {'baseline_storage_sha': storage_sha, 'baseline_source_sha': manifest['commit'], 'current': now, 'baseline': previous,
            'diff_status': status, 'policy_passed': passed, 'detail': detail}


def compare(github):
    binding = bind(github)
    with tempfile.TemporaryDirectory(prefix='bazel-fork-coverage-') as temporary:
        directory = Path(temporary)
        report_artifact(github, binding, directory)
        comparison = {'version': 1, 'binding': asdict(binding), **compare_reports(github, binding, directory)}
        require(bind(github) == binding)
        encoded = json.dumps(comparison, ensure_ascii=True, separators=(',', ':'))
        require(len(encoded) <= MAX_RESULT)
        with Path(os.environ['GITHUB_OUTPUT']).open('a') as output:
            output.write('comparison=' + encoded + '\n')
        with Path(os.environ['GITHUB_STEP_SUMMARY']).open('a') as summary:
            summary.write(comment_body(comparison))
        return 0 if comparison['policy_passed'] else 1


def local_binding(github):
    require(os.environ['GITHUB_EVENT_NAME'] == 'pull_request')
    event = unique_json(Path(os.environ['GITHUB_EVENT_PATH']).read_bytes())
    require(repository(event['repository']) == (github.repo_id, github.repo))
    number = integer(event['number'])
    expected = event['pull_request']
    pr = github.request(f'/pulls/{number}')
    require(pr['id'] == expected['id'] and pr['number'] == number and pr['state'] == 'open' and pr['merged'] is False)
    require(repository(pr['base']['repo']) == repository(expected['base']['repo']) == (github.repo_id, github.repo))
    require(repository(pr['head']['repo']) == repository(expected['head']['repo']) and pr['head']['repo']['id'] != github.repo_id)
    require(pr['head']['ref'] == expected['head']['ref'])
    base_ref = pr['base']['ref']
    require(isinstance(base_ref, str) and 0 < len(base_ref) <= 1024 and base_ref == expected['base']['ref'])
    head, base, measured = sha(pr['head']['sha']), sha(pr['base']['sha']), sha(pr['merge_commit_sha'])
    require(head == expected['head']['sha'] and base == expected['base']['sha'] and measured == sha(os.environ['GITHUB_SHA']))
    commit = github.request(f'/git/commits/{measured}')
    require(commit['sha'] == measured and [row['sha'] for row in commit['parents']] == [base, head])
    require(isinstance(pr['labels'], list) and all(isinstance(row['name'], str) for row in pr['labels']))
    return Measurement(github.repo, github.repo_id, integer(int(os.environ['GITHUB_RUN_ID'])), integer(int(os.environ['GITHUB_RUN_ATTEMPT'])),
                       number, base_ref, head, base, measured, any(row['name'] == 'rust:coverage-baseline-reset' for row in pr['labels']))


def fork_gate(github):
    binding = local_binding(github)
    root = Path(os.environ['GITHUB_WORKSPACE']).resolve()
    with tempfile.TemporaryDirectory(prefix='bazel-fork-gate-') as temporary:
        directory = Path(temporary)
        report = root / 'rsworkspace/coverage-bazel.xml'
        require(report.is_file() and not report.is_symlink() and report.stat().st_size <= MAX_REPORT)
        (directory / 'coverage-bazel.xml').write_bytes(report.read_bytes())
        comparison = compare_reports(github, binding, directory)
        require(local_binding(github) == binding)
        with Path(os.environ['GITHUB_STEP_SUMMARY']).open('a') as summary:
            summary.write(f'Bazel fork coverage: {"PASS" if comparison["policy_passed"] else "FAIL"}. Minimum coverage: 95%.\n\n'
                          f'Measured merge: `{binding.measured_sha}`.\n\n'
                          f'Counts: `{comparison["current"]}`. Main baseline counts: `{comparison["baseline"]}`.\n\n'
                          f'Baseline source: `{comparison["baseline_source_sha"]}`; storage: `{comparison["baseline_storage_sha"]}`.\n')
        return 0 if comparison['policy_passed'] else 1


def comment_body(comparison):
    binding = comparison['binding']
    def percent(counts):
        return float(Fraction(100 * (counts['statements'] - counts['misses']), counts['statements']))
    detail = html.escape(comparison['detail']).replace('@', '&#64;')
    return (f'{MARKER}\n<!-- source:{binding["run_number"]}:{binding["run_attempt"]} -->\n'
            f'## Bazel fork coverage: {"PASS" if comparison["policy_passed"] else "FAIL"}\n\n'
            f'Coverage: **{percent(comparison["current"]):.8f}%** (minimum 95%). '
            f'Main baseline: **{percent(comparison["baseline"]):.8f}%**.\n\n'
            f'PR head: `{binding["head_sha"]}`. Measured merge: `{binding["measured_sha"]}`.\n\n'
            f'Baseline source: `{comparison["baseline_source_sha"]}`; storage: `{comparison["baseline_storage_sha"]}`.\n\n'
            f'[Source run](https://github.com/{binding["repository"]}/actions/runs/{binding["run_id"]}/attempts/{binding["run_attempt"]}). '
            f'Baseline reset: `{str(binding["reset"]).lower()}`.\n\n'
            f'<details><summary>Diff against main</summary><pre>{detail}</pre></details>\n')


def publish(github):
    encoded = os.environ['COVERAGE_COMPARISON']
    require(len(encoded) <= MAX_RESULT)
    comparison = unique_json(encoded)
    require(set(comparison) == {'version', 'binding', 'baseline_storage_sha', 'baseline_source_sha', 'current', 'baseline', 'diff_status', 'policy_passed', 'detail'})
    require(type(comparison['version']) is int and comparison['version'] == 1)
    binding = bind(github)
    require(isinstance(comparison['binding'], dict))
    for name in ('repository_id', 'consumer_run', 'consumer_attempt', 'run_id', 'run_attempt', 'run_number', 'workflow_id', 'pr', 'pr_id', 'head_repository_id'):
        integer(comparison['binding'][name])
    require(type(comparison['binding']['reset']) is bool)
    require(comparison['binding'] == asdict(binding))
    sha(comparison['baseline_storage_sha'])
    sha(comparison['baseline_source_sha'])
    require(type(comparison['policy_passed']) is bool and comparison['policy_passed'] == policy(comparison['current'], comparison['baseline'], comparison['diff_status'], binding.reset))
    require(isinstance(comparison['detail'], str) and len(comparison['detail']) <= MAX_DETAIL + 100)
    comments = github.pages(f'/issues/{binding.pr}/comments')
    matches = [row for row in comments if row['user']['login'] == 'github-actions[bot]' and row['user']['type'] == 'Bot' and row['body'].startswith(MARKER + '\n')]
    require(len(matches) <= 1)
    if matches:
        match = re.match(re.escape(MARKER) + r'\n<!-- source:(\d+):(\d+) -->\n', matches[0]['body'])
        require(match and tuple(map(int, match.groups())) <= (binding.run_number, binding.run_attempt))
    body = comment_body(comparison)
    require(len(body.encode()) < 60000)
    require(bind(github) == binding)
    endpoint = f'/issues/comments/{integer(matches[0]["id"])}' if matches else f'/issues/{binding.pr}/comments'
    github.request(endpoint, method='PATCH' if matches else 'POST', body={'body': body})
    return 0


def main(command):
    try:
        return {'compare': compare, 'publish': publish, 'fork-gate': fork_gate}[command](GitHub())
    except (KeyError, ValueError, TypeError, AttributeError, IndexError, OSError, UnicodeError, subprocess.SubprocessError, zipfile.BadZipFile, RuntimeError):
        print('Fork coverage refused malformed, unavailable, or stale input.', file=sys.stderr)
        return 1
