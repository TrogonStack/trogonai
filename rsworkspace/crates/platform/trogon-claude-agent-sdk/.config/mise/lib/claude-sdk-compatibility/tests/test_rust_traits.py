import unittest

from gate_errors import GateError
from rust_surface import extract
from rustdoc_fixture import add, document, function, generics, implementation, node, path


def trait_document():
    doc = document()
    add(doc, node(1, "Value", "struct", {"kind": "unit", "generics": generics(), "impls": [5, 6, 7, 8]}))
    add(doc, node(2, "Convert", "trait", {"is_auto": False, "is_unsafe": False, "is_dyn_compatible": True,
        "items": [3, 4], "generics": generics(), "bounds": [], "implementations": [5]}))
    required = function(path(1))
    required["has_body"] = False
    add(doc, node(3, "convert", "function", required, "default"), False)
    add(doc, node(4, "Output", "assoc_type", {"generics": generics(), "bounds": [], "type": None}, "default"), False)
    for identifier in (5, 6, 7, 8):
        add(doc, node(identifier, None, "impl", implementation(path(1), [3, 4], path(2)["resolved_path"])), False)
    doc["index"]["6"]["inner"]["impl"]["is_synthetic"] = True
    doc["index"]["7"]["inner"]["impl"]["blanket_impl"] = {"generic": "T"}
    doc["index"]["8"]["attrs"] = ["automatically_derived"]
    return doc

