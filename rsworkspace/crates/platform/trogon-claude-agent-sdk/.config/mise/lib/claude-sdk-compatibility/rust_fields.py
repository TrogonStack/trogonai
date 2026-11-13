from rust_references import require


def fields(reader, identifiers, public_only=False):
    require(isinstance(identifiers, list), "rust-fields")
    result = {}
    for identifier in identifiers:
        item = reader.item(identifier)
        require(set(item["inner"]) == {"struct_field"}, "rust-field")
        if public_only and not reader.is_public(item):
            continue
        name = item["name"]
        require(isinstance(name, str) and name not in result, "rust-field-name")
        result[name] = reader.type(item["inner"]["struct_field"])
    return result


def tuple_fields(reader, identifiers):
    require(isinstance(identifiers, list), "rust-tuple-fields")
    result = []
    for identifier in identifiers:
        if identifier is None:
            result.append(None)
        else:
            item = reader.item(identifier)
            require(set(item["inner"]) == {"struct_field"}, "rust-field")
            result.append(reader.type(item["inner"]["struct_field"]))
    return result


