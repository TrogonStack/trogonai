import ast
import unittest
from upstream_python import required_wrappers

class WrapperIdentityTests(unittest.TestCase):
    def test_wrapper_identity_tracks_module_bindings_and_evaluated_defaults(self):
        cases = [
            ('NR = list', None), ('NR: object = list', None), ('del NR', None),
            ('class NR:\n    pass', None), ('def NR():\n    pass', None),
            ('import collections as NR', None), ('from .typing import NotRequired as NR', None),
            ('for NR in items:\n    pass', None), ('with context as NR:\n    pass', None),
            ('values = [(NR := list) for item in items]', None),
            ('values = [(lambda x=(NR := list): None) for item in items]', None),
            ('value = lambda x=(NR := list): None', None),
            ('NR: object', False), ('def local():\n    NR = list', False),
            ('class Local:\n    NR = list', False), ('values = [NR for NR in items]', False),
            ('NR = list\nfrom typing import Required as NR', True),
        ]
        for binding, expected in cases:
            with self.subTest(binding=binding):
                tree = ast.parse('from typing import NotRequired as NR\n' + binding)
                self.assertEqual(required_wrappers(tree), {'NR': expected})
