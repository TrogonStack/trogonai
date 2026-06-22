// edition:2024

#[derive(Debug)]
struct DomainError;

impl std::fmt::Display for DomainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("domain failure")
    }
}

#[allow(manual_error_impl)] impl std::error::Error for DomainError {}

#[derive(Debug)]
struct DomainValue;

struct MessagePair(String, u8);

impl std::fmt::Display for DomainValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("domain value")
    }
}

fn direct_probe(error: DomainError) -> bool {
    error.to_string().contains("domain")
}

fn alias_probe(error: DomainError) -> bool {
    let message = error.to_string();
    message == "domain failure"
}

fn tuple_destructure_probe(error: DomainError) -> bool {
    let (message, _) = (error.to_string(), 7);
    message.contains("domain")
}

fn at_pattern_probe(error: DomainError) -> bool {
    let ref whole @ ref inner = error.to_string();
    whole.contains("domain") || inner.contains("domain")
}

fn reassignment_probe(error: DomainError) -> bool {
    let mut message = String::new();
    message = error.to_string();
    message.contains("domain")
}

fn argument_probe(error: DomainError) -> bool {
    "domain failure".contains(&error.to_string())
}

fn let_chain_probe(error: DomainError) -> bool {
    if true && let message = error.to_string() {
        message.contains("domain")
    } else {
        false
    }
}

fn if_initializer_probe(error: DomainError, use_error: bool) -> bool {
    let message = if use_error {
        error.to_string()
    } else {
        String::new()
    };
    message.contains("domain")
}

fn match_initializer_probe(error: DomainError, use_error: bool) -> bool {
    let message = match use_error {
        true => error.to_string(),
        false => String::new(),
    };
    message.contains("domain")
}

fn tuple_struct_initializer_probe(error: DomainError) -> bool {
    let MessagePair(message, _) = MessagePair(error.to_string(), 7);
    message.contains("domain")
}

fn clone_forwarder_probe(error: DomainError) -> bool {
    let message = error.to_string().clone();
    message.contains("domain")
}

fn closure_capture_probe(error: DomainError) -> bool {
    let message = error.to_string();
    let closure = || message.contains("domain");
    closure()
}

fn slice_suffix_probe(error: DomainError) -> bool {
    let [_, .., message] = [String::new(), error.to_string()];
    message.contains("domain")
}

fn deserialization_assertion_shape(error: DomainError) {
    assert!(error.to_string().contains("caller_id"));
}

fn enum_variant_display_shape() {
    assert!(DomainError.to_string().contains("domain"));
}

fn multiline_display_shape(error: DomainError) -> bool {
    error
        .to_string()
        .contains("domain")
}

fn auth_tampered_signature_shape(error: DomainError) {
    assert!(
        error.to_string().contains("decode")
            || error.to_string().contains("verify")
            || error.to_string().contains("signature")
    );
}

fn normal_string(value: DomainValue) -> bool {
    value.to_string().contains("domain")
}

fn message_pair_from_call(_: String, value: u8) -> MessagePair {
    MessagePair(String::new(), value)
}

fn tuple_struct_return_probe(error: DomainError) -> bool {
    let MessagePair(message, _) = message_pair_from_call(error.to_string(), 7);
    message.contains("domain")
}

fn main() {
    let _ = direct_probe(DomainError);
    let _ = alias_probe(DomainError);
    let _ = tuple_destructure_probe(DomainError);
    let _ = at_pattern_probe(DomainError);
    let _ = reassignment_probe(DomainError);
    let _ = argument_probe(DomainError);
    let _ = let_chain_probe(DomainError);
    let _ = if_initializer_probe(DomainError, true);
    let _ = match_initializer_probe(DomainError, true);
    let _ = tuple_struct_initializer_probe(DomainError);
    let _ = clone_forwarder_probe(DomainError);
    let _ = closure_capture_probe(DomainError);
    let _ = slice_suffix_probe(DomainError);
    deserialization_assertion_shape(DomainError);
    enum_variant_display_shape();
    let _ = multiline_display_shape(DomainError);
    auth_tampered_signature_shape(DomainError);
    let _ = normal_string(DomainValue);
    let _ = tuple_struct_return_probe(DomainError);
}
