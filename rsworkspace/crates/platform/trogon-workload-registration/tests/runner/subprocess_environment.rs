pub(super) fn preserve_coverage(command: &mut std::process::Command) {
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
}
