import os
import unittest
from pathlib import Path

from check_policies import expected_fixture

# Not resolved: under Bazel this file is a runfiles symlink, and the fixture is
# found through the runfiles tree, not through where the symlink points.
REVIEWED_FIXTURE = Path(os.path.abspath(__file__)).parents[3] / "goworkspace/internal/spirepolicy/testdata/deployment.json"


class ReviewedPolicyFixtureTests(unittest.TestCase):
    def test_go_policy_fixture_matches_the_generated_deployment(self):
        # The Go admission tests evaluate this reviewed copy, so a stale one
        # would pass them against resources the deployment no longer renders.
        if REVIEWED_FIXTURE.read_bytes() != expected_fixture():
            self.fail("Go policy fixture is stale; regenerate with check_policies.py --write-fixture and review the diff")


if __name__ == "__main__":
    unittest.main()
