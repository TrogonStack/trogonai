use crate::install_command::install_command;
use crate::install_context::InstallContext;

/// Install instructions, surfaced verbatim in error messages.
pub fn install_instructions(ctx: &InstallContext, force: bool) -> String {
    format!(
        "Windows sandbox needs a one-time install (one UAC prompt):\n  {}\nNo logout is needed: \
         the WFP filter keys on the dedicated `trogon-sandbox` user's SID, so your network is \
         unaffected.",
        install_command(ctx, force)
    )
}
