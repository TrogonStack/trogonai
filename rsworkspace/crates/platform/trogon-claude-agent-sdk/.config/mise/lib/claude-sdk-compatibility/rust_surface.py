from gate_errors import format_boundary
from rust_data import data_shape
from rust_exports import inventory


@format_boundary("rustdoc")
def extract(document):
    reader, exports = inventory(document)
    result = {}
    for name, node in exports.items():
        kind, body = next(iter(node["inner"].items()))
        result[name] = data_shape(reader, name, kind, body)
    return result
