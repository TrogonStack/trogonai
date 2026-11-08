use trogonai_proto::agents::agents::v1alpha1::EditableSurface;
use trogonai_proto::evaluation::bindings::{FindingTargetCase, v1alpha1::Finding};

/// A settings path covers itself and its whole subtree, where segments are dot separated field
/// names. `a.b` covers `a.b` and `a.b.c`, never `a.bc`.
pub(crate) fn path_covers(listed: &str, path: &str) -> bool {
    match path.strip_prefix(listed) {
        Some("") => true,
        Some(rest) => rest.starts_with('.'),
        None => false,
    }
}

pub(crate) fn finding_inside_surface(finding: &Finding, surface: &EditableSurface) -> bool {
    match finding.target.as_option().and_then(|target| target.target.as_ref()) {
        Some(FindingTargetCase::SettingsPath(path)) => surface
            .settings_paths
            .iter()
            .any(|listed| path_covers(&listed.path, path)),
        Some(FindingTargetCase::PlatformComponent(component)) => {
            surface.platform_components.iter().any(|listed| listed == component)
        }
        None => false,
    }
}
