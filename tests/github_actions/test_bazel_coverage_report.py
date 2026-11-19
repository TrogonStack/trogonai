"""Keep generated bindings out of coverage without hiding handwritten adapters."""

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from xml.etree import ElementTree as ET


ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT / '.config/mise/tasks/github/workflows/ci-bazel/coverage-report'


class BazelCoverageReportTest(unittest.TestCase):
    def test_generated_session_bindings_follow_the_existing_codegen_policy(self):
        with tempfile.TemporaryDirectory(prefix='bazel-coverage-report-') as temporary:
            root = Path(temporary)
            config = root / 'rsworkspace/.cargo/config.toml'
            config.parent.mkdir(parents=True)
            config.write_bytes((ROOT / 'rsworkspace/.cargo/config.toml').read_bytes())
            sources = {
                'crates/platform/trogonai-proto/src/gen/binding.rs': 0,
                'crates/platform/trogonai-proto/src/session_gen/binding.rs': 0,
                'crates/platform/trogonai-proto/src/session_contracts.rs': 0,
                'crates/session/trogon-session-store/src/runtime.rs': 1,
                'crates/example/src/session_gen/handwritten.rs': 0,
            }
            records = []
            for name, hits in sources.items():
                path = root / 'rsworkspace' / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('pub fn example() {}\n')
                records.append(f'SF:rsworkspace/{name}\nDA:1,{hits}\nend_of_record\n')
            lcov = root / 'coverage.dat'
            lcov.write_text(''.join(records))
            output = root / 'coverage.xml'
            subprocess.run(
                [sys.executable, str(REPORT), str(lcov), str(output)],
                cwd=root,
                env={**os.environ, 'MISE_PROJECT_ROOT': str(root)},
                check=True,
                capture_output=True,
                text=True,
            )
            report = ET.parse(output).getroot()
            classes = {node.get('filename'): node for node in report.findall('.//class')}
            self.assertEqual(set(classes), {
                'crates/platform/trogonai-proto/src/session_contracts.rs',
                'crates/session/trogon-session-store/src/runtime.rs',
                'crates/example/src/session_gen/handwritten.rs',
            })
            self.assertEqual(report.get('lines-valid'), '3')
            self.assertEqual(report.get('lines-covered'), '1')
            self.assertEqual(
                classes['crates/platform/trogonai-proto/src/session_contracts.rs'].find('lines/line').get('hits'),
                '0',
            )


if __name__ == '__main__':
    unittest.main()
