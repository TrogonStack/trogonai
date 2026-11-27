/// What the profile read grant is about to do, in one line, for the person at
/// the keyboard.
pub fn profile_grant_notice(profile_dir: &str) -> String {
    format!(
        "Making {profile_dir} readable to sandboxed commands on this machine, except the \
         excluded credential stores (SSH and GPG keys, cloud and browser sign-ins, password \
         managers, registry hives). Readable files include package-manager configuration that \
         can hold feed tokens: .npmrc, pip.ini, NuGet.Config, Maven settings.xml, \
         gradle.properties, .gitconfig. Skip with --skip-profile-grant; undo with `fence-helper \
         profile forget`."
    )
}
