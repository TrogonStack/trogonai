#!/usr/bin/env python3

import argparse
import json
from pathlib import Path
import subprocess
import tempfile
import tomllib


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    arguments = parser.parse_args()
    root = Path(__file__).resolve().parents[3]
    pins = tomllib.loads((root / ".config/mise/config.toml").read_text())["tools"]
    runtime = tomllib.loads((root / "rsworkspace/Cargo.toml").read_text())["workspace"]["dependencies"]
    family = {runtime[name]["version"].removeprefix("=") for name in ("buffa", "buffa-types", "buffa-descriptor")}
    family.update(pins[f"cargo:{name}"] for name in ("protoc-gen-buffa", "protoc-gen-buffa-packaging"))
    if len(family) != 1:
        raise SystemExit("Buffa generators and runtime must use one exact version")

    def executable(name, tool):
        return subprocess.check_output(["mise", "which", name, "--tool", tool], cwd=root, text=True).strip()

    def run(command):
        subprocess.run(command, cwd=root, check=True)

    buf = executable("buf", f"buf@{pins['buf']}")
    if subprocess.check_output([buf, "--version"], text=True).strip() != pins["buf"]:
        raise SystemExit("Buf version does not match the repository pin")
    template = (root / ".config/buf/buf.session.gen.yaml").read_text()
    input_paths = [line.removeprefix("      - ") for line in template.splitlines() if line.startswith("      - proto/")]
    if not input_paths or len(input_paths) != len(set(input_paths)):
        raise SystemExit("Session generation requires distinct explicit protobuf input paths")
    descriptor_paths = [argument for path in input_paths for argument in ("--path", path)]
    output_name = "rsworkspace/crates/platform/trogonai-proto/src/session_gen"
    if template.count("directory: proto\n") != 1 or template.count(f"out: {output_name}\n") != 2:
        raise SystemExit("Session generation template changed")
    template = template.replace("directory: proto\n", f"directory: {json.dumps(str(root / 'proto'))}\n")
    for name in ("protoc-gen-buffa", "protoc-gen-buffa-packaging"):
        version = pins[f"cargo:{name}"]
        plugin = executable(name, f"cargo:{name}@{version}")
        if subprocess.check_output([plugin, "--version"], text=True).strip() != f"{name} {version}":
            raise SystemExit(f"{name} version does not match the repository pin")
        template = template.replace(f"local: [.config/mise/lib/proto-plugin, {name}]", f"local: {json.dumps(plugin)}")
    rust = pins["rust"]
    rust_version = rust["version"] if isinstance(rust, dict) else rust
    rustfmt = executable("rustfmt", f"rust@{rust_version}")
    with tempfile.TemporaryDirectory(prefix="session-codegen-") as temporary:
        output = Path(temporary) / "generated"
        config = Path(temporary) / "buf.gen.yaml"
        config.write_text(template.replace(f"out: {output_name}\n", f"out: {json.dumps(str(output))}\n"))
        run([buf, "generate", "--template", str(config)])
        run([buf, "build", "proto", "--as-file-descriptor-set", "--exclude-source-info", *descriptor_paths, "-o", str(output / "contracts.json")])
        run([
            buf, "build", "proto", "--as-file-descriptor-set", "--exclude-source-info",
            "--path", "proto/trogonai/session/plans",
            "--path", "proto/trogonai/agents/agents/v1alpha1/agent.proto",
            "-o", str(output / "plan-contracts.json"),
        ])
        run([rustfmt, "--edition", "2024", "--config-path", str(root / "rsworkspace/rustfmt.toml"), str(output / "mod.rs")])
        expected = root / output_name
        generated_files = {p.relative_to(output) for p in output.rglob("*") if p.is_file()}
        existing_files = {p.relative_to(expected) for p in expected.rglob("*") if p.is_file()}
        if arguments.check:
            differences = generated_files.symmetric_difference(existing_files)
            differences.update(p for p in generated_files & existing_files if (output / p).read_bytes() != (expected / p).read_bytes())
            if differences:
                names = ", ".join(str(p) for p in sorted(differences)[:10])
                raise SystemExit(f"Session generated output differs: {names}")
            print(f"Verified {len(generated_files)} Session generated files")
        else:
            for relative in existing_files - generated_files:
                (expected / relative).unlink()
            for relative in generated_files:
                destination = expected / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes((output / relative).read_bytes())
            print(f"Generated {len(generated_files)} Session files with pinned tools")


if __name__ == "__main__":
    main()
