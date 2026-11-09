from rust_exports import inventory


def extract(document):
    reader, exports = inventory(document)
    return {name: {"kind": next(iter(node["inner"]))} for name, node in exports.items()}
