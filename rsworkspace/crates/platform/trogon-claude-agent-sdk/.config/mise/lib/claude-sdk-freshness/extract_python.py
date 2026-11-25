"""Inspect the Python SDK without importing or evaluating package code."""

from __future__ import annotations

import argparse
import ast
import json
import sys
from dataclasses import dataclass, field
from pathlib import Path


class ExtractionError(ValueError):
    pass


def syntax(node: ast.AST | None) -> str | None:
    return None if node is None else ast.unparse(node)


@dataclass(frozen=True)
class ImportReference:
    module: str
    name: str | None
    external: bool


@dataclass
class Module:
    name: str
    path: Path
    tree: ast.Module
    bindings: dict[str, list[ast.AST | ImportReference]] = field(default_factory=dict)


class Inventory:
    def __init__(self, root: Path):
        self.root = root.resolve()
        self.package = self.root / "src" / "claude_agent_sdk"
        if not (self.package / "__init__.py").is_file():
            raise ExtractionError("expected src/claude_agent_sdk/__init__.py")
        self.modules: dict[str, Module] = {}
        self.symbols: dict[str, dict] = {}
        self.exported: set[tuple[str, str]] = set()
        self.queued: list[tuple[Module, str, str, list[ast.AST]]] = []
        self.emitted: set[tuple[str, str]] = set()

    def source(self, module: Module, node: ast.AST) -> dict:
        return {"path": module.path.relative_to(self.root).as_posix(), "line": node.lineno}

    def module(self, name: str) -> Module:
        if name in self.modules:
            return self.modules[name]
        candidate = self.package.joinpath(*name.split(".")) if name else self.package
        path = candidate.with_suffix(".py") if name and candidate.with_suffix(".py").is_file() else candidate / "__init__.py"
        if not path.is_file():
            raise ExtractionError(f"unresolved local module: {name or '<root>'}")
        for component in (path, *path.parents):
            if component == self.root:
                break
            if component.is_symlink():
                raise ExtractionError("symbolic links are not supported in Python source packages")
        if not path.resolve().is_relative_to(self.root):
            raise ExtractionError("local Python module escapes source package")
        try:
            tree = ast.parse(path.read_text(encoding="utf-8"), filename=path.relative_to(self.root).as_posix())
        except (SyntaxError, UnicodeError) as error:
            raise ExtractionError(f"cannot parse {path.relative_to(self.root)}") from error
        result = Module(name, path, tree)
        self.modules[name] = result
        package = name if path.name == "__init__.py" else name.rpartition(".")[0]
        for node in self.statements(tree.body):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                result.bindings.setdefault(node.name, []).append(node)
            elif isinstance(node, ast.Assign):
                for target in node.targets:
                    if isinstance(target, ast.Name):
                        result.bindings[target.id] = [node]
            elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
                result.bindings[node.target.id] = [node]
            elif isinstance(node, getattr(ast, "TypeAlias", ())):
                result.bindings[node.name.id] = [node]
            elif isinstance(node, ast.ImportFrom):
                if node.level:
                    parts = package.split(".") if package else []
                    if node.level > len(parts) + 1:
                        raise ExtractionError("relative import escapes SDK package")
                    base = parts[: len(parts) + 1 - node.level]
                    imported = ".".join(base + ([node.module] if node.module else []))
                    external = False
                elif node.module == "claude_agent_sdk" or (node.module or "").startswith("claude_agent_sdk."):
                    imported = (node.module or "").removeprefix("claude_agent_sdk").lstrip(".")
                    external = False
                else:
                    imported, external = node.module or "", True
                for alias in node.names:
                    if alias.name == "*":
                        if external:
                            raise ExtractionError("cannot resolve external star import statically")
                        source = self.module(imported)
                        for exported in self.export_names(source):
                            result.bindings[exported] = [ImportReference(imported, exported, False)]
                    else:
                        result.bindings[alias.asname or alias.name] = [ImportReference(imported, alias.name, external)]
            elif isinstance(node, ast.Import):
                for alias in node.names:
                    external = not (alias.name == "claude_agent_sdk" or alias.name.startswith("claude_agent_sdk."))
                    imported = alias.name if external else alias.name.removeprefix("claude_agent_sdk").lstrip(".")
                    result.bindings[alias.asname or alias.name.split(".")[0]] = [ImportReference(imported, None, external)]
        return result

    @staticmethod
    def statements(nodes: list[ast.stmt]):
        for node in nodes:
            if isinstance(node, (ast.If, ast.Try, ast.TryStar)):
                yield from Inventory.statements(node.body)
                yield from Inventory.statements(node.orelse)
                if isinstance(node, (ast.Try, ast.TryStar)):
                    yield from Inventory.statements(node.finalbody)
                    for handler in node.handlers:
                        yield from Inventory.statements(handler.body)
            else:
                yield node

    def export_names(self, module: Module) -> list[str]:
        names = None
        literals = {}

        def sequence(expression):
            if isinstance(expression, ast.Name) and expression.id in literals:
                return literals[expression.id]
            if isinstance(expression, ast.BinOp) and isinstance(expression.op, ast.Add):
                return sequence(expression.left) + sequence(expression.right)
            try:
                result = ast.literal_eval(expression)
            except (ValueError, TypeError) as error:
                raise ExtractionError("__all__ must be a static string sequence") from error
            if not isinstance(result, (tuple, list)) or not all(isinstance(name, str) for name in result):
                raise ExtractionError("__all__ must be a static string sequence")
            return list(result)

        for node in module.tree.body:
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                continue
            mentions_exports = any(isinstance(item, ast.Name) and item.id == "__all__" for item in ast.walk(node))
            if isinstance(node, (ast.Assign, ast.AnnAssign)):
                targets = node.targets if isinstance(node, ast.Assign) else [node.target]
                if any(isinstance(target, ast.Name) and target.id == "__all__" for target in targets):
                    if any(not isinstance(target, ast.Name) for target in targets):
                        raise ExtractionError("unsupported __all__ assignment")
                    names = sequence(node.value)
                    literals["__all__"] = names
                    continue
                for target in targets:
                    if isinstance(target, ast.Name) and node.value is not None:
                        try:
                            literals[target.id] = sequence(node.value)
                        except ExtractionError:
                            literals.pop(target.id, None)
            if isinstance(node, ast.AugAssign) and isinstance(node.target, ast.Name) and node.target.id == "__all__" and isinstance(node.op, ast.Add) and names is not None:
                names = names + sequence(node.value)
                literals["__all__"] = names
                continue
            if isinstance(node, ast.Expr) and isinstance(node.value, ast.Call):
                call = node.value
                if isinstance(call.func, ast.Attribute) and isinstance(call.func.value, ast.Name) and call.func.value.id == "__all__" and names is not None and len(call.args) == 1 and not call.keywords:
                    if call.func.attr == "append":
                        if not isinstance(call.args[0], ast.Constant) or not isinstance(call.args[0].value, str):
                            raise ExtractionError("__all__.append requires a literal string")
                        names = names + [call.args[0].value]
                    elif call.func.attr == "extend":
                        names = names + sequence(call.args[0])
                    else:
                        raise ExtractionError("unsupported __all__ mutation")
                    literals["__all__"] = names
                    continue
            if mentions_exports:
                raise ExtractionError("unsupported or conditional __all__ mutation")
        if names is not None:
            if len(names) != len(set(names)):
                raise ExtractionError("duplicate name in __all__")
            return names
        if not module.name:
            raise ExtractionError("root API has no static __all__")
        return sorted(name for name in module.bindings if not name.startswith("_"))

    def resolve(self, module: Module, name: str, seen: set[tuple[str, str]] | None = None):
        seen = set() if seen is None else set(seen)
        key = (module.name, name)
        if key in seen:
            raise ExtractionError(f"cyclic local re-export: {module.name}.{name}")
        seen.add(key)
        nodes = module.bindings.get(name)
        if nodes is None:
            raise ExtractionError(f"unresolved local export: {module.name}.{name}")
        if len(nodes) == 1 and isinstance(nodes[0], ImportReference):
            reference = nodes[0]
            if reference.external or reference.name is None:
                return reference
            return self.resolve(self.module(reference.module), reference.name, seen)
        return module, name, nodes

    def put(self, name: str, kind: str, shape: dict, module: Module, node: ast.AST):
        record = {"kind": kind, "shape": shape, "source": self.source(module, node)}
        previous = self.symbols.get(name)
        if previous is not None and previous != record:
            raise ExtractionError(f"conflicting public symbol: {name}")
        self.symbols[name] = record

    @staticmethod
    def signature(node: ast.FunctionDef | ast.AsyncFunctionDef) -> dict:
        positional = [*node.args.posonlyargs, *node.args.args]
        defaults = [None] * (len(positional) - len(node.args.defaults)) + list(node.args.defaults)
        arguments = []
        for index, (argument, default) in enumerate(zip(positional, defaults)):
            arguments.append({"name": argument.arg, "kind": "positional-only" if index < len(node.args.posonlyargs) else "positional-or-keyword", "annotation": syntax(argument.annotation), "default": syntax(default)})
        if node.args.vararg:
            arguments.append({"name": node.args.vararg.arg, "kind": "variadic", "annotation": syntax(node.args.vararg.annotation)})
        for argument, default in zip(node.args.kwonlyargs, node.args.kw_defaults):
            arguments.append({"name": argument.arg, "kind": "keyword-only", "annotation": syntax(argument.annotation), "default": syntax(default)})
        if node.args.kwarg:
            arguments.append({"name": node.args.kwarg.arg, "kind": "keyword-variadic", "annotation": syntax(node.args.kwarg.annotation)})
        return {"async": isinstance(node, ast.AsyncFunctionDef), "arguments": arguments, "returns": syntax(node.returns), "decorators": [syntax(item) for item in node.decorator_list], "type_parameters": [syntax(item) for item in getattr(node, "type_params", [])]}

    def typed_dict(self, module: Module, node: ast.ClassDef, seen: set[tuple[str, str]] | None = None) -> bool:
        seen = set() if seen is None else set(seen)
        key = (module.name, node.name)
        if key in seen:
            return False
        seen.add(key)
        for base in node.bases:
            if syntax(base) in ("TypedDict", "typing.TypedDict", "typing_extensions.TypedDict"):
                return True
            if isinstance(base, ast.Name) and base.id in module.bindings:
                resolved = self.resolve(module, base.id)
                if not isinstance(resolved, ImportReference):
                    owner, _, declarations = resolved
                    if len(declarations) == 1 and isinstance(declarations[0], ast.ClassDef) and self.typed_dict(owner, declarations[0], seen):
                        return True
        return False

    def members(self, module: Module, node: ast.ClassDef, seen: set[tuple[str, str]] | None = None) -> dict[str, tuple[Module, list[ast.AST], ast.ClassDef]]:
        seen = set() if seen is None else set(seen)
        key = (module.name, node.name)
        if key in seen:
            raise ExtractionError(f"cyclic local inheritance: {module.name}.{node.name}")
        seen.add(key)
        result = {}
        for base in node.bases:
            target = base.value if isinstance(base, ast.Subscript) else base
            if isinstance(target, ast.Name) and target.id in module.bindings:
                resolved = self.resolve(module, target.id)
                if not isinstance(resolved, ImportReference):
                    owner, _, declarations = resolved
                    if len(declarations) == 1 and isinstance(declarations[0], ast.ClassDef):
                        for name, member in self.members(owner, declarations[0], seen).items():
                            result.setdefault(name, member)
        own: dict[str, list[ast.AST]] = {}
        for member in self.statements(node.body):
            if isinstance(member, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                if not member.name.startswith("_") or member.name in ("__init__", "__call__", "__aiter__", "__anext__", "__enter__", "__exit__", "__aenter__", "__aexit__"):
                    own.setdefault(member.name, []).append(member)
            elif isinstance(member, ast.AnnAssign) and isinstance(member.target, ast.Name) and not member.target.id.startswith("_"):
                own[member.target.id] = [member]
            elif isinstance(member, ast.Assign):
                for target in member.targets:
                    if isinstance(target, ast.Name) and not target.id.startswith("_"):
                        own[target.id] = [member]
        result.update({name: (module, declarations, node) for name, declarations in own.items()})
        return result

    @staticmethod
    def reference_nodes(node: ast.AST) -> list[ast.AST]:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            return [node.args, *node.decorator_list, *([node.returns] if node.returns else []), *getattr(node, "type_params", [])]
        if isinstance(node, ast.ClassDef):
            return [*node.bases, *node.keywords, *node.decorator_list, *getattr(node, "type_params", [])]
        return [node]

    def references(self, module: Module, nodes: list[ast.AST]):
        for declaration in nodes:
            for reference in self.reference_nodes(declaration):
                names = {child.id for child in ast.walk(reference) if isinstance(child, ast.Name)}
                for child in ast.walk(reference):
                    if isinstance(child, ast.Constant) and isinstance(child.value, str):
                        try:
                            forward = ast.parse(child.value, mode="eval")
                        except SyntaxError:
                            continue
                        names.update(item.id for item in ast.walk(forward) if isinstance(item, ast.Name))
                for name in sorted(names & module.bindings.keys()):
                    resolved = self.resolve(module, name)
                    if isinstance(resolved, ImportReference):
                        continue
                    owner, original, declarations = resolved
                    identity = (owner.name, original)
                    if identity not in self.exported and identity not in self.emitted:
                        self.emitted.add(identity)
                        self.queued.append((owner, original, f"{owner.name}.{original}" if owner.name else original, declarations))

    def emit(self, module: Module, name: str, path: str, nodes: list[ast.AST], context: ast.ClassDef | None = None):
        node = nodes[-1]
        if all(isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)) for item in nodes):
            self.put(path, "method" if context else "function", {"signatures": [self.signature(item) for item in nodes]}, module, nodes[0])
        elif len(nodes) != 1:
            raise ExtractionError(f"conflicting declarations: {path}")
        elif isinstance(node, ast.ClassDef):
            self.put(path, "class", {"bases": [syntax(base) for base in node.bases], "keywords": {item.arg or "**": syntax(item.value) for item in node.keywords}, "decorators": [syntax(item) for item in node.decorator_list], "type_parameters": [syntax(item) for item in getattr(node, "type_params", [])]}, module, node)
            for member, (owner, declarations, declaring_class) in self.members(module, node).items():
                self.emit(owner, member, f"{path}.{member}", declarations, declaring_class)
        elif isinstance(node, ast.AnnAssign):
            annotation, default = node.annotation, node.value
            total = not context or not any(keyword.arg == "total" and isinstance(keyword.value, ast.Constant) and keyword.value.value is False for keyword in context.keywords)
            required = default is None
            if context and self.typed_dict(module, context):
                required = total
                if isinstance(annotation, ast.Subscript):
                    marker = syntax(annotation.value).rpartition(".")[2]
                    if marker in ("Required", "NotRequired"):
                        required = marker == "Required"
            if isinstance(default, ast.Call) and syntax(default.func).rpartition(".")[2] == "field":
                required = not any(item.arg in ("default", "default_factory") for item in default.keywords)
            self.put(path, "field" if context else "type-alias" if syntax(annotation) in ("TypeAlias", "typing.TypeAlias") else "value", {"annotation": syntax(annotation), "default": syntax(default), "required": required}, module, node)
        elif isinstance(node, ast.Assign):
            self.put(path, "field" if context else "assignment", {"value": syntax(node.value)}, module, node)
        elif isinstance(node, getattr(ast, "TypeAlias", ())):
            self.put(path, "type-alias", {"value": syntax(node.value), "type_parameters": [syntax(item) for item in node.type_params]}, module, node)
        else:
            self.put(path, "opaque", {"syntax": ast.dump(node, include_attributes=False)}, module, node)
        self.references(module, nodes)

    def extract(self) -> dict:
        root = self.module("")
        exports = self.export_names(root)
        if not exports:
            raise ExtractionError("root API has no exports")
        resolved = {}
        for name in exports:
            value = self.resolve(root, name)
            resolved[name] = value
            if not isinstance(value, ImportReference):
                owner, original, _ = value
                self.exported.add((owner.name, original))
        for name in sorted(resolved):
            value = resolved[name]
            if isinstance(value, ImportReference):
                self.symbols[name] = {"kind": "external-reference", "shape": {"module": value.module, "name": value.name}, "source": {"path": root.path.relative_to(self.root).as_posix(), "line": 1}}
            else:
                owner, original, declarations = value
                self.emit(owner, original, name, declarations)
        while self.queued:
            owner, original, path, declarations = self.queued.pop(0)
            self.emit(owner, original, path, declarations)
        for module in self.modules.values():
            self.symbols[f"modules.{module.path.relative_to(self.package).as_posix()}.imports"] = {
                "kind": "module-imports",
                "shape": {"declarations": [syntax(node) for node in self.statements(module.tree.body) if isinstance(node, (ast.Import, ast.ImportFrom))]},
                "source": {"path": module.path.relative_to(self.root).as_posix(), "line": 1},
            }
        return {"schema_version": 1, "language": "python", "symbols": dict(sorted(self.symbols.items()))}


def extract(root: Path) -> dict:
    return Inventory(Path(root)).extract()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    arguments = parser.parse_args()
    try:
        inventory = extract(arguments.root)
    except (ExtractionError, OSError) as error:
        print(f"Python API extraction failed: {error}", file=sys.stderr)
        return 1
    json.dump(inventory, sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
