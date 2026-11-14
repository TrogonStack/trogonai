"""Content-free provenance and non-executing, bounded Git object access."""

from contextlib import contextmanager
from dataclasses import dataclass
from decimal import Decimal, InvalidOperation
from xml.parsers import expat
from diagnostics import CoverageError, Reason
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import tempfile
from xml.etree import ElementTree as ET

MAX_REPORT = 128 * 1024 * 1024
MAX_SOURCE = 16 * 1024 * 1024
MAX_FILES = 20000


def digest(data):
    return hashlib.sha256(data).hexdigest()


def identity(value):
    if not isinstance(value, str) or not re.fullmatch(r'[0-9a-f]{40}', value):
        raise ValueError('Expected immutable SHA-1 Git object identity')
    return value


def repository_name(value):
    if not isinstance(value, str) or not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.-]*/[A-Za-z0-9][A-Za-z0-9_.-]*', value):
        raise ValueError('Invalid trusted repository identity')
    return value.lower()


def relative_path(value):
    if not isinstance(value, str) or len(value) > 4096 or not value:
        raise ValueError('Invalid relative path')
    path = PurePosixPath(value)
    if path.is_absolute() or path.as_posix() != value or any(p.casefold() in ('.', '..', '.git') for p in path.parts) or not path.parts or any(ord(c) < 32 or c in '\\:' for c in value):
        raise ValueError('Invalid relative path')
    return value


def bounded_read(path, limit=MAX_REPORT):
    with Path(path).open('rb') as stream:
        data = stream.read(limit + 1)
    if len(data) > limit:
        raise ValueError('Coverage input exceeds size limit')
    return data


def manifest_json(data):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError('Duplicate manifest key')
            result[key] = value
        return result
    return json.loads(data, object_pairs_hook=unique)


def xml_tree(data):
    builder = ET.TreeBuilder()
    parser = expat.ParserCreate()
    def declaration(name, system, public, internal):
        if name != 'coverage' or public or internal or system not in (
            'http://cobertura.sourceforge.net/xml/coverage-04.dtd',
            'https://cobertura.sourceforge.net/xml/coverage-04.dtd',
        ):
            raise CoverageError(Reason.XML_DECLARATION)
    def entity(*args):
        raise CoverageError(Reason.XML_DECLARATION)
    parser.StartDoctypeDeclHandler = declaration
    parser.EntityDeclHandler = entity
    parser.ExternalEntityRefHandler = entity
    parser.StartElementHandler = builder.start
    parser.EndElementHandler = builder.end
    parser.CharacterDataHandler = builder.data
    try:
        parser.Parse(data, True)
        root = builder.close()
    except expat.ExpatError as error:
        raise ValueError('Invalid XML') from error
    if root.tag != 'coverage':
        raise ValueError('Expected Cobertura coverage')
    if len(root.findall('.//class')) > MAX_FILES:
        raise ValueError('Too many coverage classes')
    return root


def git(root, *args):
    # No inherited Git configuration, credential helpers, hooks, replace objects,
    # alternate object directories, URL rewrites, or executable remote helpers.
    environment = {k: os.environ[k] for k in ('PATH', 'SYSTEMROOT', 'TMPDIR') if k in os.environ}
    environment.update(GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL=os.devnull,
                       GIT_TERMINAL_PROMPT='0', GIT_NO_REPLACE_OBJECTS='1',
                       GIT_CONFIG_COUNT='0', GIT_LFS_SKIP_SMUDGE='1', GIT_NO_LAZY_FETCH='1')
    command = ['git', '-c', 'core.hooksPath=' + os.devnull, '-c', 'credential.helper=',
               '-c', 'protocol.allow=never', '-c', 'protocol.https.allow=always',
               '-c', 'http.followRedirects=false', '-c', 'http.lowSpeedLimit=1024',
               '-c', 'http.lowSpeedTime=30', '-C', str(root), *args]
    result = subprocess.run(command, env=environment, capture_output=True, timeout=120)
    if result.returncode:
        raise ValueError('Required Git object operation failed')
    return result.stdout


@dataclass(frozen=True)
class GitObjects:
    directory: Path
    commit: str

    def __post_init__(self):
        identity(self.commit)
        keys = git(self.directory, 'config', '--local', '--name-only', '--list').decode().lower().splitlines()
        if any(key == 'extensions.partialclone' or (key.startswith('remote.') and key.endswith('.promisor')) for key in keys):
            raise ValueError('Partial clone object stores are not supported')
        if git(self.directory, 'cat-file', '-t', self.commit).strip() != b'commit':
            raise ValueError('Expected a commit object')

    def source(self, path):
        relative_path(path)
        record = git(self.directory, 'ls-tree', '-z', self.commit, '--', path)
        rows = record.split(b'\0')
        if len(rows) != 2 or rows[-1] or b'\t' not in rows[0]:
            raise ValueError('Source is absent from commit')
        metadata, stored_path = rows[0].split(b'\t', 1)
        mode, kind, blob = metadata.decode().split()
        if stored_path.decode() != path or mode not in ('100644', '100755') or kind != 'blob':
            raise ValueError('Source must be a regular Git blob')
        blob = identity(blob)
        if int(git(self.directory, 'cat-file', '-s', blob)) > MAX_SOURCE:
            raise ValueError('Source exceeds size limit')
        data = git(self.directory, 'cat-file', 'blob', blob)
        if hashlib.sha1(b'blob ' + str(len(data)).encode() + b'\0' + data).hexdigest() != blob:
            raise ValueError('Git blob identity mismatch')
        data.decode('utf-8')
        return blob, data


def context_identity(commit):
    base = repository_name(os.environ['GITHUB_REPOSITORY'])
    if commit == os.environ.get('GITHUB_SHA'):
        return base
    if commit == os.environ.get('COVERAGE_HEAD_SHA'):
        return repository_name(os.environ['COVERAGE_HEAD_REPOSITORY'])
    raise ValueError('Checkout commit does not match trusted action context')


@contextmanager
def trusted_objects(repository, commit):
    """Use context-anchored local history or fetch only from the expected public repo."""
    repository = repository_name(repository)
    identity(commit)
    root = Path(os.environ['GITHUB_WORKSPACE'])
    anchors = []
    if repository == repository_name(os.environ['GITHUB_REPOSITORY']):
        anchors.extend([os.environ.get('GITHUB_SHA'), os.environ.get('COVERAGE_BASE_SHA')])
    if os.environ.get('COVERAGE_HEAD_REPOSITORY') and repository == repository_name(os.environ['COVERAGE_HEAD_REPOSITORY']):
        anchors.append(os.environ.get('COVERAGE_HEAD_SHA'))
    # The workflow checkout is trusted only at an event-provided immutable anchor.
    # No manifest-provided URL, local directory, ref, token, or fetch option is used.
    for anchor in anchors:
        if not anchor:
            continue
        try:
            identity(anchor)
            git(root, 'merge-base', '--is-ancestor', commit, anchor)
            objects = GitObjects(root, commit)
        except ValueError:
            continue
        yield objects
        return
    with tempfile.TemporaryDirectory(prefix='coverage-git-') as temporary:
        directory = Path(temporary)
        git(directory, 'init', '--bare', '--template=')
        git(directory, 'fetch', '--no-tags', '--depth=1', '--no-recurse-submodules',
            'https://github.com/' + repository + '.git', commit)
        if git(directory, 'rev-parse', 'FETCH_HEAD').decode().strip() != commit:
            raise ValueError('Fetched commit mismatch')
        yield GitObjects(directory, commit)


def numeric(attributes, keys):
    result = {}
    for key in keys:
        if key in attributes:
            value = attributes[key]
            pattern = r'\d+(?:\.\d+)?(?:[eE][+-]?\d+)?' if key.endswith('-rate') else r'\d+'
            if len(value) > 40 or not re.fullmatch(pattern, value):
                raise ValueError('Invalid numeric coverage attribute')
            try:
                if key.endswith('-rate') and not 0 <= Decimal(value) <= 1:
                    raise ValueError('Invalid numeric coverage rate')
            except InvalidOperation as error:
                raise ValueError('Invalid numeric coverage rate') from error
            result[key] = value
    return result


def canonical_xml(tree, paths):
    """Keep measured class lines/branches and numeric totals, drop producer metadata."""
    totals = ('line-rate', 'branch-rate', 'lines-covered', 'lines-valid', 'branches-covered', 'branches-valid')
    result = ET.Element('coverage', numeric(tree.attrib, totals))
    ET.SubElement(ET.SubElement(result, 'sources'), 'source').text = '.'
    packages = ET.SubElement(result, 'packages')
    classes = ET.SubElement(ET.SubElement(packages, 'package', {'name': 'coverage'}), 'classes')
    for original in tree.findall('.//class'):
        path = relative_path(paths[original.attrib['filename']])
        node = ET.SubElement(classes, 'class', {'name': path, 'filename': path, **numeric(original.attrib, totals)})
        lines = ET.SubElement(node, 'lines')
        for line in original.findall('./lines/line'):
            attrs = numeric(line.attrib, ('number', 'hits'))
            if set(attrs) != {'number', 'hits'} or not all(v.isdigit() for v in attrs.values()) or int(attrs['number']) < 1:
                raise ValueError('Invalid measured line')
            if 'branch' in line.attrib:
                if line.attrib['branch'] not in ('true', 'false'):
                    raise ValueError('Invalid branch indicator')
                attrs['branch'] = line.attrib['branch']
            if 'condition-coverage' in line.attrib:
                value = line.attrib['condition-coverage']
                if not re.fullmatch(r'\d+(?:\.\d+)?% \(\d+/\d+\)', value):
                    raise ValueError('Invalid branch coverage')
                attrs['condition-coverage'] = value
            target = ET.SubElement(lines, 'line', attrs)
            for condition in line.findall('./conditions/condition'):
                attrs = numeric(condition.attrib, ('number',))
                kind = condition.get('type', '')
                coverage = condition.get('coverage', '')
                if kind not in ('jump', 'switch') or not re.fullmatch(r'\d+(?:\.\d+)?%', coverage):
                    raise ValueError('Unsupported branch condition')
                attrs.update(type=kind, coverage=coverage)
                conditions = target.find('conditions')
                if conditions is None:
                    conditions = ET.SubElement(target, 'conditions')
                ET.SubElement(conditions, 'condition', attrs)
    return ET.tostring(result, encoding='utf-8', xml_declaration=True) + b'\n'


def validate_lines(node, contents):
    count = len(contents.decode('utf-8').splitlines())
    if any(not 1 <= int(line.attrib['number']) <= count for line in node.findall('./lines/line')):
        raise ValueError('Coverage line is outside its source file')


def checkout_autocrlf(repository):
    # Only this read-only scalar query sees user config; object and transport
    # operations must never inherit its hooks, helpers, or URL rewrites.
    environment = {k: os.environ[k] for k in (
        'PATH', 'SYSTEMROOT', 'TMPDIR', 'HOME', 'XDG_CONFIG_HOME', 'GIT_CONFIG_GLOBAL',
    ) if k in os.environ}
    environment.update(GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_COUNT='0',
                       GIT_TERMINAL_PROMPT='0', GIT_NO_REPLACE_OBJECTS='1')
    result = subprocess.run([
        'git', '-C', str(repository), 'config', '--includes', '--type=bool',
        '--default=false', '--get', 'core.autocrlf',
    ], env=environment, capture_output=True, timeout=120)
    return result.returncode == 0 and result.stdout.strip() == b'true'


def checkout_matches(repository, path, working, contents):
    if working == contents:
        return True
    if b'\r' in contents or b'\n' in working.replace(b'\r\n', b'') or working.replace(b'\r\n', b'\n') != contents:
        return False
    attributes = git(repository, 'check-attr', '-z', 'text', 'eol', '--', path).decode().split('\0')
    values = dict(zip(attributes[1::3], attributes[2::3]))
    if values.get('text') == 'unset':
        return False
    return values.get('eol') == 'crlf' or (values.get('eol') == 'unspecified' and checkout_autocrlf(repository))


def make_pair(report, output, repository, objects, paths, working_root=None):
    tree = xml_tree(bounded_read(report))
    sources = {}
    for node in tree.findall('.//class'):
        path = relative_path(paths[node.attrib['filename']])
        blob, contents = objects.source(path)
        if working_root is not None:
            working = working_root / path
            if working.resolve() != working or working.is_symlink() or not checkout_matches(objects.directory, path, bounded_read(working, MAX_SOURCE), contents):
                raise CoverageError(Reason.SOURCE_MISMATCH)
        validate_lines(node, contents)
        sources[path] = {'path': path, 'sha256': digest(contents), 'blob': blob}
    xml = canonical_xml(tree, paths)
    manifest = {'version': 2, 'repository': repository_name(repository), 'commit': objects.commit,
                'report_sha256': digest(xml), 'sources': sources}
    output = Path(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.with_name('coverage.xml').write_bytes(xml)
    output.write_text(json.dumps(manifest, sort_keys=True) + '\n')


def capture(report, output, source_root):
    report = Path(report).resolve()
    root = Path(source_root or os.environ.get('GITHUB_WORKSPACE', '.')).resolve()
    repo = Path(git(root, 'rev-parse', '--show-toplevel').decode().strip()).resolve()
    commit = git(repo, 'rev-parse', 'HEAD').decode().strip()
    repository = context_identity(commit)
    tree = xml_tree(bounded_read(report))
    roots = [root] if source_root else [repo, report.parent, *(Path(n.text) for n in tree.findall('./sources/source') if n.text)]
    paths = {}
    for node in tree.findall('.//class'):
        filename = node.attrib['filename']
        candidates = {base / filename for base in roots if (base / filename).is_file()}
        if any(p.is_symlink() or p.absolute() != p.resolve() for p in candidates):
            raise ValueError('Source aliases or symbolic links are not supported')
        candidates = {p.absolute() for p in candidates}
        if len(candidates) != 1:
            raise ValueError('Missing or ambiguous coverage source; set source-root')
        paths[filename] = candidates.pop().relative_to(repo).as_posix()
    make_pair(report, output, repository, GitObjects(repo, commit), paths, repo)


def load_manifest(report, manifest, expected_repository):
    xml = bounded_read(report)
    data = manifest_json(bounded_read(manifest))
    if set(data) != {'version', 'repository', 'commit', 'report_sha256', 'sources'} or data['version'] != 2:
        raise ValueError('Expected content-free version 2 manifest')
    if data['repository'] != repository_name(expected_repository) or data['report_sha256'] != digest(xml):
        raise CoverageError(Reason.MANIFEST_BINDING)
    identity(data['commit'])
    tree = xml_tree(xml)
    names = {n.attrib['filename'] for n in tree.findall('.//class')}
    if names != set(data['sources']):
        raise ValueError('Manifest and report file sets differ')
    if canonical_xml(tree, {name: name for name in names}) != xml:
        raise ValueError('Report is not canonical publication format')
    for name, source in data['sources'].items():
        relative_path(name)
        if set(source) != {'path', 'sha256', 'blob'} or source['path'] != name or not re.fullmatch(r'[0-9a-f]{64}', source['sha256']):
            raise ValueError('Invalid manifest source entry')
        identity(source['blob'])
    return tree, data


def materialize(report, manifest, destination, repository, provider=trusted_objects):
    tree, data = load_manifest(report, manifest, repository)
    classes = {name: [] for name in data['sources']}
    for node in tree.findall('.//class'):
        classes[node.attrib['filename']].append(node)
    with provider(repository, data['commit']) as objects:
        for name, source in data['sources'].items():
            blob, contents = objects.source(name)
            if blob != source['blob'] or digest(contents) != source['sha256']:
                raise ValueError('Source does not match the immutable Git blob')
            for node in classes[name]:
                validate_lines(node, contents)
            path = destination / 'source' / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(contents)
    (destination / 'source').mkdir(parents=True, exist_ok=True)
    (destination / 'coverage.xml').write_bytes(bounded_read(report))


def migrate(report, legacy, output, repository, commit, object_directory):
    """Offline administrator operation; a legacy snapshot alone is never authority."""
    commit = identity(commit)
    old = manifest_json(bounded_read(legacy, 256 * 1024 * 1024))
    if old['version'] != 1 or old['commit'] != commit or repository_name(old['repository']) != repository_name(repository) or old['report_sha256'] != digest(bounded_read(report)):
        raise ValueError('Legacy evidence does not match independently verified identity')
    objects = GitObjects(Path(object_directory), commit)
    tree = xml_tree(bounded_read(report))
    if set(old['sources']) != {n.attrib['filename'] for n in tree.findall('.//class')}:
        raise ValueError('Legacy file set mismatch')
    paths = {}
    for name, source in old['sources'].items():
        path = relative_path(source['path'])
        _, contents = objects.source(path)
        if source['sha256'] != digest(contents) or source['text'].encode('utf-8') != contents:
            raise ValueError('Legacy source differs from verified immutable blob')
        paths[name] = path
    make_pair(report, output, repository, objects, paths)
