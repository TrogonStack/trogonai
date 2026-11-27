/// Scoped by atespace as well as by name so one tenant cannot address another's entry, even
/// if a future naming scheme were to allow the same actor name in two atespaces.
pub fn entry_key(atespace: &str, actor: &str) -> String {
    format!("{atespace}.{actor}")
}
