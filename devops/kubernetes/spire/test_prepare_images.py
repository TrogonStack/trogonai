import subprocess
import tempfile
import unittest
from pathlib import Path

from prepare_images import prepare


class BuildSourceTrustTests(unittest.TestCase):
    def test_unverified_release_cannot_produce_a_build_context(self):
        def refused(command):
            return subprocess.CompletedProcess(command, 1, "", "unavailable")

        with tempfile.TemporaryDirectory() as scratch:
            output = Path(scratch) / "output"
            with self.assertRaisesRegex(ValueError, "artifact verification failed"):
                prepare("arm64", output, Path(scratch), refused)
            self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
