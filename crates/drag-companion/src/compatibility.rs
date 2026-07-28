use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if requests_human_output(&arguments) {
        eprintln!(
            "warning: `drag-companion` is deprecated; use `drag tracking` or `drag-tracking`"
        );
    }

    let executable = tracking_executable();
    match Command::new(&executable).args(&arguments).status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!(
                "error: could not start `drag-tracking` from compatibility shim: {error}; reinstall Drag so both executables are available"
            );
            std::process::exit(1);
        }
    }
}

fn requests_human_output(arguments: &[OsString]) -> bool {
    arguments
        .windows(2)
        .any(|pair| pair[0] == OsStr::new("--output") && pair[1] == OsStr::new("human"))
        || arguments
            .iter()
            .any(|argument| argument == OsStr::new("--output=human"))
}

fn tracking_executable() -> PathBuf {
    let name = if cfg!(windows) {
        "drag-tracking.exe"
    } else {
        "drag-tracking"
    };
    std::env::current_exe()
        .ok()
        .and_then(|current| current.parent().map(|parent| parent.join(name)))
        .filter(|adjacent| adjacent.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}
