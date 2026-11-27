use crate::install_command::install_command;
use crate::install_context::InstallContext;
use crate::sandbox_user_status::SandboxUserStatus;
use crate::sublayer_guid::SublayerGuid;

/// Why an existing install cannot be used by this `fence-helper` (its setup marker
/// is another release's; re-run the install), or `None`. An `fence-helper` that
/// does not report its setup version is not judged.
pub fn setup_version_mismatch(user: &SandboxUserStatus, sublayer_guid: Option<&SublayerGuid>) -> Option<String> {
    let setup = user.setup_version?;
    if user.marker_version == Some(setup) {
        return None;
    }
    if let Some(marker) = user.marker_version
        && marker > setup
    {
        let sublayer = sublayer_guid
            .map(|guid| format!(" --sublayer-guid {guid}"))
            .unwrap_or_default();
        return Some(format!(
            "The Windows sandbox on this machine was set up by a newer release (setup version \
             {marker}; this release needs {setup}). Use that release, or replace its install \
             with this one: `fence-helper.exe install --force{sublayer}`.\nNothing is granted or run \
             until then."
        ));
    }
    let command = install_command(
        &InstallContext {
            sublayer_guid: sublayer_guid.cloned(),
            ..InstallContext::default()
        },
        false,
    );
    let marker = user
        .marker_version
        .map_or_else(|| "unknown".to_string(), |v| v.to_string());
    Some(format!(
        "The Windows sandbox on this machine was set up by a different release (setup version \
         {marker}; this release needs {setup}). Re-run the install to upgrade it in place (one \
         UAC prompt):\n  {command}\nNothing is granted or run until then."
    ))
}
