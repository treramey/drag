use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::cli::{
    TrackingArgs, TrackingCommand, TrackingReviewCommand, TrackingScheduleCommand,
    TrackingSourcesCommand, TrackingSubmissionMode,
};
use crate::tracking_setup::{TrackingOnboardingOutcome, TrackingOnboardingSession};
use crate::tracking_setup_tui::LineTrackingOnboardingSession;
use crate::{CliError, ResolvedOutputMode};

const TRACKING_CONTRACT_VERSION: u64 = 3;

pub(crate) fn run(
    mut args: TrackingArgs,
    mode: ResolvedOutputMode,
    config_path: &std::path::Path,
) -> Result<u8, CliError> {
    if let TrackingCommand::Setup(setup) = &args.command {
        let session = LineTrackingOnboardingSession::terminal();
        if implicit_interactive_setup(setup) && session.is_terminal() {
            args.command = match session.run()? {
                TrackingOnboardingOutcome::Configure(plan) => {
                    TrackingCommand::Setup(plan.into_args())
                }
                TrackingOnboardingOutcome::Declined => {
                    emit_interactive_setup_outcome(
                        mode,
                        "declined",
                        "Automatic tracking setup declined. Run `drag tracking setup` whenever you are ready.",
                    )?;
                    return Ok(0);
                }
                TrackingOnboardingOutcome::Cancelled => {
                    emit_interactive_setup_outcome(
                        mode,
                        "cancelled",
                        "Automatic tracking setup cancelled; no tracking choices were applied.",
                    )?;
                    return Ok(0);
                }
            };
        }
    }
    let executable = tracking_executable();
    verify_contract(&executable)?;
    let drag_executable = std::env::current_exe().map_err(|error| {
        tracking_unavailable(format!(
            "could not locate the invoking Drag executable: {error}"
        ))
    })?;

    let mut command = Command::new(&executable);
    command
        .args(["--output", output_name(mode)])
        .arg("--drag-bin")
        .arg(drag_executable)
        .env("DRAG_CONFIG", config_path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    match args.command {
        TrackingCommand::Capture => {
            return Err(CliError::Invariant(
                "Claude capture was not handled locally".to_owned(),
            ));
        }
        TrackingCommand::Setup(args) => {
            append_setup_args(&mut command, args);
        }
        TrackingCommand::Status => {
            command.arg("status");
        }
        TrackingCommand::Run(args) => {
            command.arg("run");
            if let Some(when) = args.when {
                command.arg(when);
            }
        }
        TrackingCommand::Review(args) => {
            command.arg("review");
            match args.operation {
                Some(TrackingReviewCommand::Approve(date)) => {
                    command.arg("approve");
                    if let Some(when) = date.when {
                        command.arg(when);
                    }
                }
                None => {
                    if let Some(when) = args.when {
                        command.arg(when);
                    }
                    if args.approve {
                        command.arg("--approve");
                    }
                }
            }
        }
        TrackingCommand::Pause => {
            command.arg("pause");
        }
        TrackingCommand::Resume => {
            command.arg("resume");
        }
        TrackingCommand::Uninstall => {
            command.arg("uninstall");
        }
        TrackingCommand::Sources(args) => {
            command.arg("sources");
            match args.operation {
                TrackingSourcesCommand::List => {
                    command.arg("list");
                }
                TrackingSourcesCommand::Configure(args) => {
                    command.arg("configure");
                    for repo in args.repos {
                        command.args(["--repo".as_ref(), repo.as_os_str()]);
                    }
                    for file in args.ics_files {
                        command.args(["--ics".as_ref(), file.as_os_str()]);
                    }
                    if args.clear_repos {
                        command.arg("--clear-repos");
                    }
                    if args.clear_ics {
                        command.arg("--clear-ics");
                    }
                    if args.claude_code {
                        command.arg("--claude-code");
                    }
                    if args.no_claude_code {
                        command.arg("--no-claude-code");
                    }
                }
                TrackingSourcesCommand::Test(args) => {
                    command.arg("test");
                    if let Some(when) = args.when {
                        command.arg(when);
                    }
                }
            }
        }
        TrackingCommand::Schedule(args) => {
            command.arg("schedule");
            match args.operation {
                TrackingScheduleCommand::Show => {
                    command.arg("show");
                }
                TrackingScheduleCommand::Update(args) => {
                    command.args(["update", "--at", &args.at]);
                    if let Some(timezone) = args.schedule_timezone {
                        command.args(["--schedule-timezone", &timezone]);
                    }
                }
                TrackingScheduleCommand::Pause => {
                    command.arg("pause");
                }
                TrackingScheduleCommand::Resume => {
                    command.arg("resume");
                }
            }
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

fn emit_interactive_setup_outcome(
    mode: ResolvedOutputMode,
    status: &str,
    human: &str,
) -> Result<(), CliError> {
    if mode == ResolvedOutputMode::Human {
        writeln!(std::io::stdout().lock(), "{human}")?;
    } else {
        serde_json::to_writer(
            std::io::stdout().lock(),
            &serde_json::json!({
                "ok": true,
                "data": {
                    "status": status,
                    "configured": false,
                    "nextCommand": "drag tracking setup"
                }
            }),
        )?;
        writeln!(std::io::stdout().lock())?;
    }
    Ok(())
}

fn append_setup_args(command: &mut Command, args: crate::cli::TrackingSetupArgs) {
    command.arg("setup");
    if let Some(mode) = args.mode {
        command.args(["--mode", submission_mode_name(mode)]);
    }
    if args.authorize_automatic {
        command.arg("--authorize-automatic");
    }
    if args.install_scheduler {
        command.arg("--install-scheduler");
    }
    if args.install_hooks {
        command.arg("--install-hooks");
    }
    if let Some(target) = args.scheduler_target {
        command.args(["--scheduler-target".as_ref(), target.as_os_str()]);
    }
    if let Some(at) = args.at {
        command.args(["--at", &at]);
    }
    if let Some(timezone) = args.schedule_timezone {
        command.args(["--schedule-timezone", &timezone]);
    }
    for repo in args.repos {
        command.args(["--repo".as_ref(), repo.as_os_str()]);
    }
    if args.clear_repos {
        command.arg("--clear-repos");
    }
    for file in args.ics_files {
        command.args(["--ics".as_ref(), file.as_os_str()]);
    }
    if args.clear_ics {
        command.arg("--clear-ics");
    }
}

fn implicit_interactive_setup(args: &crate::cli::TrackingSetupArgs) -> bool {
    args.mode.is_none()
        && !args.authorize_automatic
        && !args.install_scheduler
        && !args.install_hooks
        && args.scheduler_target.is_none()
        && args.at.is_none()
        && args.schedule_timezone.is_none()
        && args.repos.is_empty()
        && args.ics_files.is_empty()
        && !args.clear_repos
        && !args.clear_ics
}

fn submission_mode_name(mode: TrackingSubmissionMode) -> &'static str {
    match mode {
        TrackingSubmissionMode::Draft => "draft",
        TrackingSubmissionMode::Review => "review",
        TrackingSubmissionMode::Automatic => "automatic",
    }
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
