use super::command_inputs::RedactValueInput;

#[test]
fn a_redact_value_never_appears_in_debug_output() {
    let shown = format!("{:?}", RedactValueInput("hunter2".to_string()));
    assert!(!shown.contains("hunter2"));
}
