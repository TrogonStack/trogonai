import unittest

from render import render


class ReleaseTrustTests(unittest.TestCase):
    def test_refuses_deployment_when_release_signature_is_unavailable(self):
        def unavailable(image):
            raise RuntimeError("release verification unavailable")

        with self.assertRaisesRegex(ValueError, "release verification failed"):
            render({}, unavailable)


if __name__ == "__main__":
    unittest.main()
