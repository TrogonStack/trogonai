import unittest

from gate_errors import GateError
from rust_surface import extract
from rustdoc_fixture import add, document, function, generics, implementation, node, path


def method_document():
    doc = document()
    add(doc, node(1, "Client", "struct", {"kind": "unit", "generics": generics(), "impls": [2]}))
    add(doc, node(2, None, "impl", implementation(path(1), [3, 4, 5])), False)
    add(doc, node(3, "connect", "function", function(path(1))), False)
    add(doc, node(4, "private", "function", function(), "default"), False)
    add(doc, node(5, "VERSION", "assoc_const", {"type": {"primitive": "u8"}, "value": "1"}), False)
    return doc

