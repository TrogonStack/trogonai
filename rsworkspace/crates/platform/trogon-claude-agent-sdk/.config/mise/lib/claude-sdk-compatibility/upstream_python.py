import ast
import hashlib

from gate_errors import Code, GateError


SELECTED_ALIASES = {
    "PermissionMode",
    "SettingSource",
    "EffortLevel",
    "AssistantMessageError",
    "MessageOriginKind",
    "ContentBlock",
    "Message",
}

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
    selected = {"ClaudeAgentOptions"} | SELECTED_ALIASES
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
    aliases = {}
    for node in tree.body:
        name, value = None, None
        if isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            name, value = node.target.id, node.value
        elif isinstance(node, ast.Assign) and len(node.targets) == 1 and isinstance(node.targets[0], ast.Name):
            name, value = node.targets[0].id, node.value
        if name not in SELECTED_ALIASES:
            continue
        if name in aliases:
            raise GateError(Code.FORMAT, name)
        if value is None:
            raise GateError(Code.FORMAT, name)
        literals = _literal_items(value)
        if literals is not None:
            aliases[name] = literals
            result[name] = {"literals": literals}
            result.update({name + "." + literal: {"kind": "literal"} for literal in literals})
        elif isinstance(value, (ast.BinOp, ast.Name, ast.Subscript)):
            result[name] = {"kind": "alias", "annotation": ast.unparse(value)}
        else:
            raise GateError(Code.FORMAT, name)
    if not {"PermissionMode", "SettingSource"}.issubset(aliases):
        raise GateError(Code.FORMAT, "selected-python-aliases")
    return result
