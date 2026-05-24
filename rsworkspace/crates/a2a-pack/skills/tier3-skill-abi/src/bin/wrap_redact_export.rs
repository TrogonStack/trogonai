use std::env;
use std::path::PathBuf;
use std::process;

use walrus::{
    ExportItem, FunctionBuilder, FunctionId, Module, ModuleConfig, ValType,
};
use walrus::ir::LoadKind;

fn main() {
    let Some(input) = env::args().nth(1) else {
        eprintln!("usage: wrap-redact-export <input.wasm> <output.wasm>");
        process::exit(1);
    };
    let Some(output) = env::args().nth(2) else {
        eprintln!("usage: wrap-redact-export <input.wasm> <output.wasm>");
        process::exit(1);
    };
    if let Err(err) = wrap(PathBuf::from(input), PathBuf::from(output)) {
        eprintln!("wrap-redact-export: {err}");
        process::exit(1);
    }
}

fn wrap(input: PathBuf, output: PathBuf) -> Result<(), String> {
    let config = ModuleConfig::new();
    let mut module = Module::from_file_with_config(input, &config).map_err(|e| e.to_string())?;
    let core_id = find_export_function(&module, "redact_part_core")?;
    let core_ty = module.funcs.get(core_id).ty();
    let core_params = module.types.get(core_ty).params();
    if core_params != [ValType::I32, ValType::I32, ValType::I32] {
        return Err(format!(
            "expected redact_part_core signature (i32,i32,i32), got {core_params:?}"
        ));
    }

    let shim = build_shim(&mut module, core_id)?;
    module.exports.add("redact_part", shim);
    module.emit_wasm_file(output).map_err(|e| e.to_string())
}

fn find_export_function(module: &Module, name: &str) -> Result<FunctionId, String> {
    let export = module
        .exports
        .iter()
        .find(|export| export.name == name)
        .ok_or_else(|| format!("missing {name} export"))?;
    match export.item {
        ExportItem::Function(id) => Ok(id),
        _ => Err(format!("{name} export is not a function")),
    }
}

fn build_shim(module: &mut Module, core_id: FunctionId) -> Result<FunctionId, String> {
    let memory = module
        .memories
        .iter()
        .next()
        .ok_or("wasm module must export memory")?
        .id();
    let in_ptr = module.locals.add(ValType::I32);
    let in_len = module.locals.add(ValType::I32);
    let meta = module.locals.add(ValType::I32);
    let mut builder = FunctionBuilder::new(&mut module.types, &[ValType::I32, ValType::I32], &[ValType::I32, ValType::I32]);
    builder
        .func_body()
        .i32_const(0)
        .local_set(meta)
        .local_get(in_ptr)
        .local_get(in_len)
        .local_get(meta)
        .call(core_id)
        .local_get(meta)
        .load(
            memory,
            LoadKind::I32 { atomic: false },
            walrus::ir::MemArg { align: 2, offset: 0 },
        )
        .local_get(meta)
        .load(
            memory,
            LoadKind::I32 { atomic: false },
            walrus::ir::MemArg { align: 2, offset: 4 },
        );
    Ok(builder.finish(vec![in_ptr, in_len], &mut module.funcs))
}
