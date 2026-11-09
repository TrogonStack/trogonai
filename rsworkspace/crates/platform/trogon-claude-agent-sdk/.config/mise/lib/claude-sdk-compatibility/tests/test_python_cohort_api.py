import hashlib
import unittest
from upstream_python import extract

HEADER = "class ClaudeAgentOptions:\n    pass\nPermissionMode = Literal['default']\nSettingSource = Literal['user']\n"
def read(value):
    data = (HEADER + value).encode()
    return extract(data, hashlib.sha256(data).hexdigest())

class CohortTests(unittest.TestCase):
    def test_message_aliases_are_visible_without_executing_source(self):
        result = read("EffortLevel = Literal['low', 'high']\nContentBlock = TextBlock | ThinkingBlock\n")
        self.assertEqual(result.get('EffortLevel'), {'literals': ['high', 'low']})
        self.assertEqual(result.get('ContentBlock'), {'kind': 'alias', 'annotation': 'TextBlock | ThinkingBlock'})
