use trogon_sandbox_contract::VolumeMode;

pub(crate) fn mode_label(mode: VolumeMode) -> &'static str {
    match mode {
        VolumeMode::ReadOnly => "read_only",
        VolumeMode::ReadWrite => "read_write",
        VolumeMode::CopyOnWrite => "copy_on_write",
    }
}
