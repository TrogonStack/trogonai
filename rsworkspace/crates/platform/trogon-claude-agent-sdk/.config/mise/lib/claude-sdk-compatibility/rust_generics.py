from rust_bounds import bounds, parameters
from rust_generic_values import term
from rust_references import require
from rust_schema import schema, tagged, text, texts


def generics(reader, body):
    schema(body, ("params", "where_predicates"), "rust-generics")
    parameters(reader, body["params"])
    require(isinstance(body["where_predicates"], list), "rust-where-predicates")
    for predicate in body["where_predicates"]:
        kind, value = tagged(predicate, ("bound_predicate", "lifetime_predicate", "eq_predicate"), "rust-where-predicate")
        if kind == "bound_predicate":
            schema(value, ("type", "bounds", "generic_params"), "rust-where-bound")
            reader.type(value["type"])
            bounds(reader, value["bounds"])
            parameters(reader, value["generic_params"])
        elif kind == "lifetime_predicate":
            schema(value, ("lifetime", "outlives"), "rust-where-lifetime")
            require(text(value["lifetime"]) and texts(value["outlives"]), "rust-where-lifetime")
        else:
            schema(value, ("lhs", "rhs"), "rust-where-equality")
            reader.type(value["lhs"])
            term(reader, value["rhs"])
    return reader.normalize(body)
