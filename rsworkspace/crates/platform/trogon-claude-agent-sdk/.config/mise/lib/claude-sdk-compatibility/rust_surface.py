from gate_errors import format_boundary
from rust_data import data_shape, enum_shapes
from rust_exports import inventory


@format_boundary("rustdoc")
def extract(document):
    reader, exports = inventory(document)
    result = {}
    for name, node in exports.items():
        kind, body = next(iter(node["inner"].items()))
        if kind == "enum":
            result.update(enum_shapes(reader, name, body))
        else:
            result[name] = data_shape(reader, name, kind, body)
        result[name]["attributes"] = reader.attributes(node)
    return result
