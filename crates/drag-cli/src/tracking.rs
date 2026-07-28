use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::cli::{TrackingArgs, TrackingCommand};
use crate::{CliError, ResolvedOutputMode};

const TRACKING_CONTRACT_VERSION: u64 = 1;

pub(crate) fn run(args: TrackingArgs, mode: ResolvedOutputMode) -> Result<u8, CliError> {
    let executable = tracking_executable();
    verify_contract(&executable)?;

    let mut command = Command::new(&executable);
    command
        .args(["--output", output_name(mode)])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    match args.command {
        TrackingCommand::Status => {
            command.arg("status");
        }
    }
    let status = command.status().map_err(|error| {
        tracking_unavailable(format!(
            "could not start `{}`: {error}",
            executable.display()
        ))
    })?;
    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1))
}

fn output_name(mode: ResolvedOutputMode) -> &'static str {
    match mode {
        ResolvedOutputMode::Human => "human",
        ResolvedOutputMode::Json | ResolvedOutputMode::Ndjson => "json",
    }
}

fn verify_contract(executable: &PathBuf) -> Result<(), CliError> {
    let output = Command::new(executable)
        .arg("contract")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| {
            tracking_unavailable(format!(
                "could not find or start `{}`: {error}",
                executable.display()
            ))
        })?;
    if !output.status.success() {
        return Err(CliError::TrackingIncompatible(format!(
            "`{}` could not report its machine contract; reinstall Drag so the executables have matching versions",
            executable.display()
        )));
    }
    let contract: Value = serde_json::from_slice(&output.stdout).map_err(|_| {
        CliError::TrackingIncompatible(format!(
            "`{}` returned an invalid machine contract; reinstall Drag so the executables have matching versions",
            executable.display()
        ))
    })?;
    let version = contract.get("schemaVersion").and_then(Value::as_u64);
    let binary = contract.get("binary").and_then(Value::as_str);
    if version != Some(TRACKING_CONTRACT_VERSION) || binary != Some("drag-tracking") {
        return Err(CliError::TrackingIncompatible(format!(
            "`{}` is incompatible; expected contract version {TRACKING_CONTRACT_VERSION} from `drag-tracking`, found version {} from {}; reinstall Drag so both executables come from the same release",
            executable.display(),
            version.map_or_else(|| "missing".to_owned(), |value| value.to_string()),
            binary.unwrap_or("an unknown binary")
        )));
    }
    Ok(())
}

fn tracking_unavailable(reason: String) -> CliError {
    CliError::TrackingUnavailable(format!(
        "automatic tracking is unavailable: {reason}; reinstall Drag or ensure `drag-tracking` is available next to `drag` or on PATH"
    ))
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
