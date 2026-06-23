(module
  (memory (export "memory") 1)
  (func (export "redact_part") (param i32 i32) (result i32 i32)
    local.get 0
    local.get 1))
