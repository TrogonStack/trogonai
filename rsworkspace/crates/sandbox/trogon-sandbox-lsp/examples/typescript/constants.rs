pub(crate) const PROJECT_DIR: &str = "/workspace/project";
pub(crate) const INSTALL_TIMEOUT_SECONDS: u32 = 300;
pub(crate) const SOURCE: &str = "export function greet(name: string): string {
  return `Hello, ${name}!`;
}

const message = greet('World');
message.
";
