//! Stands in for `reg.exe` as a real program at the path the SDK spawns: it prints `hklm.txt` or `hkcu.txt` by the
//! hive the second argument names, from `mnt/c/Windows/System32` under its working folder (the run root the native
//! oracle starts it in) when that folder exists, and from beside itself otherwise. As `plutil -convert` it prints
//! its last argument's file.

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let Ok(exe) = std::env::current_exe() else {
        return ExitCode::from(2);
    };
    let Some(folder) = exe.parent() else {
        return ExitCode::from(2);
    };
    let stem = exe
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if stem == "plutil" {
        if args.get(1).map(String::as_str) != Some("-convert") {
            return ExitCode::from(1);
        }
        return match args.last().map(std::fs::read) {
            Some(Ok(bytes)) if std::io::stdout().write_all(&bytes).is_ok() => ExitCode::SUCCESS,
            _ => ExitCode::from(1),
        };
    }
    let hive = args.get(2).map(String::as_str).unwrap_or_default();
    let name = if hive.starts_with("HKLM") {
        "hklm"
    } else if hive.starts_with("HKCU") {
        "hkcu"
    } else {
        "none"
    };
    let data = std::env::current_dir()
        .map(|cwd| cwd.join("mnt/c/Windows/System32"))
        .ok()
        .filter(|folder| folder.is_dir())
        .unwrap_or_else(|| folder.to_path_buf());
    match std::fs::read(data.join(format!("{name}.txt"))) {
        Ok(bytes) if std::io::stdout().write_all(&bytes).is_ok() => ExitCode::SUCCESS,
        _ => {
            eprintln!("ERROR: The system was unable to find the specified registry key or value.");
            ExitCode::from(1)
        }
    }
}
