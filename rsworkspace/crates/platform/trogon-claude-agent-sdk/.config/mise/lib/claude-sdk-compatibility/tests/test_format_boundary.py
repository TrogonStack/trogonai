import unittest
from gate_errors import Code, GateError, format_boundary

class FormatBoundaryTests(unittest.TestCase):
    def test_structural_errors_become_typed_and_keep_the_cause(self):
        for error in (KeyError("x"), TypeError("x"), IndexError("x"), AttributeError("x"), RecursionError("x")):
            @format_boundary("fixture")
            def fail():
                raise error
            with self.assertRaises(GateError) as caught:
                fail()
            self.assertEqual((caught.exception.code, caught.exception.selector), (Code.FORMAT, "fixture"))
            self.assertIs(caught.exception.__cause__, error)

    def test_success_and_existing_typed_errors_are_retained(self):
        self.assertEqual(format_boundary("fixture")(lambda: 42)(), 42)
        error = GateError(Code.SHAPE, "Value")
        @format_boundary("fixture")
        def fail():
            raise error
        with self.assertRaises(GateError) as caught:
            fail()
        self.assertIs(caught.exception, error)
