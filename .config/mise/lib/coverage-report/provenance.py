"""Keep report publication source-bound without changing comparison policy."""
import os
from pathlib import Path
import sys

from diagnostics import diagnostic
from sources import capture, digest, GitObjects, load_manifest, validate_lines


def baseline():
    if os.environ['COVERAGE_DIFF_ENABLED'] != 'true':
        return
    report = Path('data') / os.environ['COVERAGE_DIFF_BRANCH'] / os.environ['COVERAGE_STORAGE_SUBDIRECTORY'] / 'coverage.xml'
    manifest = report.with_name('sources.json')
    tree, data = load_manifest(report, manifest, os.environ['GITHUB_REPOSITORY'])
    objects = GitObjects(Path(os.environ['GITHUB_WORKSPACE']), data['commit'])
    classes = {}
    for node in tree.findall('.//class'):
        classes.setdefault(node.attrib['filename'], []).append(node)
    for name, source in data['sources'].items():
        blob, contents = objects.source(name)
        if blob != source['blob'] or digest(contents) != source['sha256']:
            raise ValueError('Baseline source does not match its immutable Git blob')
        for node in classes[name]:
            validate_lines(node, contents)


def main():
    if sys.argv[1:] == ['capture']:
        root = Path(os.environ['GITHUB_WORKSPACE'])
        report = Path(os.environ['COVERAGE_PROVENANCE_REPORT']).resolve()
        capture(report, root / 'coverage-action/verified/sources.json', '')
    elif sys.argv[1:] == ['baseline']:
        baseline()
    else:
        raise ValueError('Expected capture or baseline')


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        diagnostic(error)
        sys.exit(1)
