/// The JavaScript runtime the TypeScript SDK runs under. Its engine words the errors the SDK raises without
/// writing them itself, such as the `TypeError` from `"method" in message` when the CLI sends an `mcp_message`
/// whose `message` is not an object.
///
/// `Node`, the default, reproduces V8 (`Cannot use 'in' operator to search for 'method' in 7`). `Bun` reproduces
/// JavaScriptCore, whose text names the bundled parameter instead of the value
/// (`e is not an Object. (evaluating '"method" in e')`). The Python dialect raises no such error.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum JsRuntime {
    #[default]
    Node,
    Bun,
}
