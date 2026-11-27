use crate::constants::MAX_RESOURCE_NAME_LEN;

/// Derives a Kubernetes DNS-1123 label from a lease id: lowercase letters and digits, every other character
/// folded to `-`, at most [`MAX_RESOURCE_NAME_LEN`] characters.
pub fn resource_name(prefix: &str, lease_id: &str) -> String {
    let slug: String = lease_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = slug.trim_matches('-');
    let base = if trimmed.is_empty() { "lease" } else { trimmed };
    format!("{prefix}-{base}").chars().take(MAX_RESOURCE_NAME_LEN).collect()
}
