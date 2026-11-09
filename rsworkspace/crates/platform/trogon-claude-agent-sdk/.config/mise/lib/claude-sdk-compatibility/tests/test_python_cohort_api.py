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

    def test_plain_message_classes_preserve_fields_and_defaults(self):
        result = read("class TextBlock:\n    text: str\nclass UserMessage:\n    content: str = 'hello'\n")
        self.assertEqual(result.get('TextBlock.text'), {'annotation': 'str', 'default': None})
        self.assertEqual(result.get('UserMessage.content'), {'annotation': 'str', 'default': "'hello'"})
        with self.assertRaises(Exception):
            read("class TextBlock(Base):\n    text: str\n")
