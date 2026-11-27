use std::path::{Path, PathBuf};

use trogon_std::env::ReadEnv;

/// Finds `name` in `bin_dir`, the directory of the running executable, then the `PATH` of `env`.
pub fn find_binary(name: &str, bin_dir: Option<&Path>, env: &impl ReadEnv) -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let path_dirs: Vec<PathBuf> = env
        .var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).collect())
        .unwrap_or_default();
    bin_dir
        .map(Path::to_path_buf)
        .into_iter()
        .chain(exe_dir)
        .chain(path_dirs)
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}
