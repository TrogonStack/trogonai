from gate_errors import format_boundary
from rust_data import data_shape, enum_shapes
from rust_exports import inventory
from rust_members import function, inherent_members, manual_traits, trait_shapes
from rust_references import require


@format_boundary("rustdoc")
def extract(document):
    reader, exports = inventory(document)
    result = {}
    for name, node in exports.items():
        kind, body = next(iter(node["inner"].items()))
        if kind == "enum":
            result.update(enum_shapes(reader, name, body))
        elif kind == "function":
            result[name] = function(reader, body)
        elif kind == "trait":
            result.update(trait_shapes(reader, name, body))
        else:
            result[name] = data_shape(reader, name, kind, body)
        result[name]["attributes"] = reader.attributes(node)
        if kind in ("struct", "enum"):
            result[name]["manual_trait_impls"] = manual_traits(reader, body)
            members = inherent_members(reader, name, body)
            require(not result.keys() & members.keys(), "duplicate-public-member")
            result.update(members)
    return result
