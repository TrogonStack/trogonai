import ast
import hashlib

from gate_errors import Code, GateError


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
    selected = {"ClaudeAgentOptions", "PermissionMode", "SettingSource"}
    for statement in tree.body:
        if isinstance(statement, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
            if statement.name in selected and statement is not options[0]:
                raise GateError(Code.FORMAT, statement.name)
        elif isinstance(statement, ast.TypeAlias) and statement.name.id in selected:
            raise GateError(Code.FORMAT, statement.name.id)
        elif isinstance(statement, (ast.Assign, ast.AnnAssign)):
            targets = statement.targets if isinstance(statement, ast.Assign) else [statement.target]
            names = {n.id for target in targets for n in ast.walk(target) if isinstance(n, ast.Name)} & selected
            if names and (len(targets) != 1 or not isinstance(targets[0], ast.Name) or "ClaudeAgentOptions" in names):
                raise GateError(Code.FORMAT, "selected-python-rebinding")
        elif isinstance(statement, (ast.Import, ast.ImportFrom)):
            if any((alias.asname or alias.name.split(".")[0]) in selected for alias in statement.names):
                raise GateError(Code.FORMAT, "selected-python-import")
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
        if name not in ("PermissionMode", "SettingSource"):
            continue
        if name in aliases or not isinstance(value, ast.Subscript) or not isinstance(value.value, ast.Name) or value.value.id != "Literal":
            raise GateError(Code.FORMAT, name)
        items = value.slice.elts if isinstance(value.slice, ast.Tuple) else [value.slice]
        if not items or not all(isinstance(v, ast.Constant) and isinstance(v.value, str) for v in items):
            raise GateError(Code.FORMAT, name)
        literals = sorted(v.value for v in items)
        if len(literals) != len(set(literals)):
            raise GateError(Code.FORMAT, name)
        aliases[name] = literals
        result[name] = {"literals": literals}
        result.update({name + "." + value: {"kind": "literal"} for value in literals})
    if set(aliases) != {"PermissionMode", "SettingSource"}:
        raise GateError(Code.FORMAT, "selected-python-aliases")
    return result
