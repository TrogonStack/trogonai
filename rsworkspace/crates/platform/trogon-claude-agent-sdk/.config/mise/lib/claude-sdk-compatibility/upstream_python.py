import ast
import hashlib

from gate_errors import Code, GateError, format_boundary


SELECTED_CLASSES = {
    "ClaudeAgentOptions",
    "TextBlock",
    "ThinkingBlock",
    "ToolUseBlock",
    "ToolResultBlock",
    "UserMessage",
    "AssistantMessage",
    "SystemMessage",
    "ResultMessage",
    "StreamEvent",
}
SELECTED_ALIASES = {
    "PermissionMode",
    "SettingSource",
    "EffortLevel",
    "AssistantMessageError",
    "MessageOriginKind",
    "ContentBlock",
    "Message",
}
SELECTED_TYPED_DICTS = {"MessageOrigin"}
SELECTED = SELECTED_CLASSES | SELECTED_ALIASES | SELECTED_TYPED_DICTS


def _literal_items(value):
    if not isinstance(value, ast.Subscript) or not isinstance(value.value, ast.Name) or value.value.id != "Literal":
        return None
    items = value.slice.elts if isinstance(value.slice, ast.Tuple) else [value.slice]
    if not items or not all(isinstance(v, ast.Constant) and isinstance(v.value, str) for v in items):
        raise GateError(Code.FORMAT, "python-literal")
    literals = sorted(v.value for v in items)
    if len(literals) != len(set(literals)):
        raise GateError(Code.FORMAT, "python-literal")
    return literals


def _field_shape(node, required=None):
    if not isinstance(node, ast.AnnAssign) or not isinstance(node.target, ast.Name) or node.simple != 1:
        raise GateError(Code.FORMAT, "python-field")
    shape = {"annotation": ast.unparse(node.annotation),
             "default": None if node.value is None else ast.unparse(node.value)}
    if required is not None:
        shape["required"] = required
    return node.target.id, shape


def _required_wrappers(tree):
    wrappers = {}

    def invalidate(name):
        if name in wrappers:
            wrappers[name] = None

    def comprehension(node):
        if isinstance(node, ast.Lambda):
            for default in (*node.args.defaults, *node.args.kw_defaults):
                if default is not None:
                    comprehension(default)
            return
        if isinstance(node, ast.NamedExpr):
            visit(node.target)
        for child in ast.iter_child_nodes(node):
            comprehension(child)

    def visit(node):
        if isinstance(node, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
            invalidate(node.name)
            for field, value in ast.iter_fields(node):
                if field != "body":
                    for child in value if isinstance(value, list) else [value]:
                        if isinstance(child, ast.AST):
                            visit(child)
            return
        if isinstance(node, ast.Lambda):
            for default in (*node.args.defaults, *node.args.kw_defaults):
                if default is not None:
                    visit(default)
            return
        if isinstance(node, (ast.ListComp, ast.SetComp, ast.DictComp, ast.GeneratorExp)):
            comprehension(node)
            return
        if isinstance(node, ast.AnnAssign) and node.value is None:
            visit(node.annotation)
            return
        if isinstance(node, ast.Name) and isinstance(node.ctx, (ast.Store, ast.Del)):
            invalidate(node.id)
        elif isinstance(node, (ast.ExceptHandler, ast.MatchAs, ast.MatchStar)):
            invalidate(node.name)
        elif isinstance(node, ast.MatchMapping):
            invalidate(node.rest)
        elif isinstance(node, ast.Import):
            for alias in node.names:
                invalidate(alias.asname or alias.name.split(".")[0])
        if isinstance(node, ast.ImportFrom):
            for alias in node.names:
                local = alias.asname or alias.name
                invalidate(local)
                if not node.level and node.module in {"typing", "typing_extensions"} and alias.name in {"Required", "NotRequired"}:
                    wrappers[local] = alias.name == "Required"
        for child in ast.iter_child_nodes(node):
            visit(child)

    visit(tree)
    return wrappers


def _required(annotation, total, wrappers):
    if isinstance(annotation, ast.Subscript) and isinstance(annotation.value, ast.Name):
        return wrappers.get(annotation.value.id, total)
    return total


def _class_shape(node, wrappers):
    if node.bases or node.type_params:
        typed_dict = any(isinstance(base, ast.Name) and base.id == "TypedDict" for base in node.bases)
        if not typed_dict:
            raise GateError(Code.FORMAT, node.name)
    decorators = [ast.unparse(value) for value in node.decorator_list]
    result = {node.name: {"kind": "typed-dict" if node.bases else "class", "decorators": decorators}}
    total = True
    for keyword in node.keywords:
        if keyword.arg == "total":
            if not isinstance(keyword.value, ast.Constant) or not isinstance(keyword.value.value, bool):
                raise GateError(Code.FORMAT, node.name)
            total = keyword.value.value
        else:
            raise GateError(Code.FORMAT, node.name)
    if node.bases:
        result[node.name]["total"] = total
    for statement in node.body:
        if isinstance(statement, ast.Expr) and isinstance(statement.value, ast.Constant) and isinstance(statement.value.value, str):
            continue
        if isinstance(statement, ast.Pass):
            continue
        name, shape = _field_shape(statement)
        if node.bases:
            shape.pop("default", None)
            annotation = statement.annotation
            shape["required"] = _required(annotation, total, wrappers)
        key = node.name + "." + name
        if key in result:
            raise GateError(Code.FORMAT, key)
        result[key] = shape
    return result


def _typed_dict_shape(name, value, wrappers):
    if not isinstance(value, ast.Call) or not isinstance(value.func, ast.Name) or value.func.id != "TypedDict":
        raise GateError(Code.FORMAT, name)
    if len(value.args) != 2 or not isinstance(value.args[0], ast.Constant) or value.args[0].value != name:
        raise GateError(Code.FORMAT, name)
    fields = value.args[1]
    if not isinstance(fields, ast.Dict) or any(key is None for key in fields.keys):
        raise GateError(Code.FORMAT, name)
    total = True
    for keyword in value.keywords:
        if keyword.arg != "total" or not isinstance(keyword.value, ast.Constant) or not isinstance(keyword.value.value, bool):
            raise GateError(Code.FORMAT, name)
        total = keyword.value.value
    result = {name: {"kind": "typed-dict", "total": total}}
    for key, annotation in zip(fields.keys, fields.values):
        if not isinstance(key, ast.Constant) or not isinstance(key.value, str):
            raise GateError(Code.FORMAT, name)
        required = _required(annotation, total, wrappers)
        result[f"{name}.{key.value}"] = {"annotation": ast.unparse(annotation), "required": required}
    return result


def reject_unreviewed_bindings(tree, selected, approved):
    def check(name, node):
        if name in selected and id(node) not in approved:
            raise GateError(Code.FORMAT, "selected-python-binding:" + name)

    def visit(node):
        if isinstance(node, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
            check(node.name, node)
            for field, value in ast.iter_fields(node):
                if field != "body":
                    for child in value if isinstance(value, list) else [value]:
                        if isinstance(child, ast.AST):
                            visit(child)
            return
        if isinstance(node, ast.Lambda):
            visit(node.args)
            return
        if isinstance(node, ast.Name) and isinstance(node.ctx, (ast.Store, ast.Del)):
            check(node.id, node)
        elif isinstance(node, (ast.Import, ast.ImportFrom)):
            for alias in node.names:
                check(alias.asname or alias.name.split(".")[0], alias)
        elif isinstance(node, (ast.ExceptHandler, ast.MatchAs, ast.MatchStar)):
            check(node.name, node)
        elif isinstance(node, ast.MatchMapping):
            check(node.rest, node)
        for child in ast.iter_child_nodes(node):
            visit(child)

    visit(tree)


@format_boundary("python-types")
def extract(contents, expected_hash):
    if len(contents) > 2 * 1024 * 1024 or hashlib.sha256(contents).hexdigest() != expected_hash:
        raise GateError(Code.PIN, "python-types")
    try:
        tree = ast.parse(contents.decode("utf-8"))
    except (UnicodeDecodeError, SyntaxError, ValueError, RecursionError) as error:
        raise GateError(Code.EXTRACTION, "python-types") from error
    options = [n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "ClaudeAgentOptions"]
    if len(options) != 1:
        raise GateError(Code.FORMAT, "ClaudeAgentOptions")
    if options[0].bases or options[0].keywords or options[0].type_params:
        raise GateError(Code.FORMAT, "ClaudeAgentOptions.inheritance-or-generics")
    selected = SELECTED
    wrappers = _required_wrappers(tree)
    approved = {id(options[0])}
    seen_selected = set()
    for statement in tree.body:
        if isinstance(statement, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
            if statement.name in SELECTED_ALIASES:
                raise GateError(Code.FORMAT, statement.name)
            if statement.name in selected and statement.name in seen_selected:
                raise GateError(Code.FORMAT, statement.name)
            if statement.name in selected:
                seen_selected.add(statement.name)
            if isinstance(statement, ast.ClassDef) and statement.name in selected:
                approved.add(id(statement))
        elif isinstance(statement, ast.TypeAlias) and statement.name.id in selected:
            raise GateError(Code.FORMAT, statement.name.id)
        elif isinstance(statement, (ast.Assign, ast.AnnAssign)):
            targets = statement.targets if isinstance(statement, ast.Assign) else [statement.target]
            names = {n.id for target in targets for n in ast.walk(target) if isinstance(n, ast.Name)} & selected
            if names and (len(targets) != 1 or not isinstance(targets[0], ast.Name) or "ClaudeAgentOptions" in names):
                raise GateError(Code.FORMAT, "selected-python-rebinding")
            if names:
                approved.add(id(targets[0]))
        elif isinstance(statement, (ast.Import, ast.ImportFrom)):
            if any((alias.asname or alias.name.split(".")[0]) in selected for alias in statement.names):
                raise GateError(Code.FORMAT, "selected-python-import")
    reject_unreviewed_bindings(tree, selected, approved)
    result = {"ClaudeAgentOptions": {"kind": "class",
                                      "decorators": [ast.unparse(node) for node in options[0].decorator_list]}}
    for node in options[0].body:
        if isinstance(node, ast.Expr) and isinstance(node.value, ast.Constant) and isinstance(node.value.value, str):
            continue
        if isinstance(node, ast.Pass):
            continue
        if not isinstance(node, ast.AnnAssign) or not isinstance(node.target, ast.Name) or node.simple != 1:
            raise GateError(Code.FORMAT, "ClaudeAgentOptions.member")
        key = "ClaudeAgentOptions." + node.target.id
        if key in result:
            raise GateError(Code.FORMAT, key)
        result[key] = {"annotation": ast.unparse(node.annotation),
                       "default": None if node.value is None else ast.unparse(node.value)}
    for node in tree.body:
        if isinstance(node, ast.ClassDef) and node.name in SELECTED_CLASSES | SELECTED_TYPED_DICTS and node.name != "ClaudeAgentOptions":
            result.update(_class_shape(node, wrappers))

    aliases = {}
    for node in tree.body:
        name, value = None, None
        if isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            name, value = node.target.id, node.value
        elif isinstance(node, ast.Assign) and len(node.targets) == 1 and isinstance(node.targets[0], ast.Name):
            name, value = node.targets[0].id, node.value
        if name not in SELECTED_ALIASES | SELECTED_TYPED_DICTS:
            continue
        if name in aliases:
            raise GateError(Code.FORMAT, name)
        if name in SELECTED_TYPED_DICTS:
            result.update(_typed_dict_shape(name, value, wrappers))
            continue
        if value is None:
            raise GateError(Code.FORMAT, name)
        literals = _literal_items(value)
        if literals is not None:
            aliases[name] = literals
            result[name] = {"literals": literals}
            result.update({name + "." + literal: {"kind": "literal"} for literal in literals})
        elif isinstance(value, (ast.BinOp, ast.Name, ast.Subscript)):
            if any(isinstance(node, ast.BinOp) and not isinstance(node.op, ast.BitOr) for node in ast.walk(value)):
                raise GateError(Code.FORMAT, name)
            result[name] = {"kind": "alias", "annotation": ast.unparse(value)}
        else:
            raise GateError(Code.FORMAT, name)
    if not {"PermissionMode", "SettingSource"}.issubset(aliases):
        raise GateError(Code.FORMAT, "selected-python-aliases")
    for name, shape in result.items():
        if "required" in shape and shape["required"] is None:
            raise GateError(Code.FORMAT, "python-required-wrapper:" + name)
    return result

required_wrappers = _required_wrappers
