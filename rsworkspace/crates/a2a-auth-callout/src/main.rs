// Stubbed in PR-a1 (scaffolding only). The real entrypoint that wires
// signing keys, dispatcher, subscriber and graceful shutdown lands with the
// final PR in this series — see `.trogonai/todos/a2a-auth-callout-pr-plan.internal.trogonai.md`.
#[cfg(not(coverage))]
fn main() {}

#[cfg(coverage)]
fn main() {}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(coverage)]
    fn coverage_main_stub_is_callable() {
        super::main();
    }
}
