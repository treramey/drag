//! Shared interactive onboarding for automatic tracking.

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use serde_json::Value;

use crate::cli::{TrackingSetupArgs, TrackingSubmissionMode};
use crate::tracking;
use crate::CliError;

const PRIVACY_EXPLANATION: &str = "Tracking reads only the local evidence sources you select. Raw evidence stays on this machine. Runs use Drag's read and preview network boundaries; worklogs can be created only when you separately choose automatic mode and authorize submission. Installing hooks, installing a schedule, and allowing mutation are independent choices.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TrackingSetupPlan {
    pub(crate) mode: TrackingSubmissionMode,
    pub(crate) authorize_automatic: bool,
    pub(crate) install_scheduler: bool,
    pub(crate) install_hooks: bool,
    pub(crate) scheduler_target: Option<PathBuf>,
    pub(crate) at: String,
    pub(crate) schedule_timezone: String,
    pub(crate) repos: Vec<PathBuf>,
    pub(crate) ics_files: Vec<PathBuf>,
}

impl TrackingSetupPlan {
    pub(crate) fn into_args(self) -> TrackingSetupArgs {
        TrackingSetupArgs {
            mode: Some(self.mode),
            authorize_automatic: self.authorize_automatic,
            install_scheduler: self.install_scheduler,
            install_hooks: self.install_hooks,
            scheduler_target: self.scheduler_target,
            at: Some(self.at),
            schedule_timezone: Some(self.schedule_timezone),
            repos: self.repos,
            ics_files: self.ics_files,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TrackingOnboardingOutcome {
    Declined,
    Cancelled,
    Configure(TrackingSetupPlan),
}

pub(crate) trait TrackingOnboardingSession: Send + Sync {
    fn is_terminal(&self) -> bool;
    fn run(&self) -> Result<TrackingOnboardingOutcome, CliError>;
}

pub(crate) trait TrackingSetupInstaller: Send + Sync {
    fn install(&self, plan: &TrackingSetupPlan) -> Result<Value, CliError>;
}

impl<T: TrackingSetupInstaller + ?Sized> TrackingSetupInstaller for std::sync::Arc<T> {
    fn install(&self, plan: &TrackingSetupPlan) -> Result<Value, CliError> {
        (**self).install(plan)
    }
}

pub(crate) struct ProcessTrackingSetupInstaller {
    config_path: PathBuf,
}

impl ProcessTrackingSetupInstaller {
    pub(crate) fn new(config_path: PathBuf) -> Self {
        Self { config_path }
    }
}

impl TrackingSetupInstaller for ProcessTrackingSetupInstaller {
    fn install(&self, plan: &TrackingSetupPlan) -> Result<Value, CliError> {
        tracking::run_setup_capture(plan.clone().into_args(), &self.config_path)
    }
}

pub(crate) trait TrackingSetupPrompter: Send + Sync {
    fn is_terminal(&self) -> bool;
    fn message(&self, message: &str) -> Result<(), CliError>;
    fn prompt(&self, label: &str, default: Option<&str>) -> Result<Option<String>, CliError>;
}

pub(crate) struct TerminalTrackingSetupPrompter;

impl TrackingSetupPrompter for TerminalTrackingSetupPrompter {
    fn is_terminal(&self) -> bool {
        io::stdin().is_terminal() && io::stderr().is_terminal()
    }

    fn message(&self, message: &str) -> Result<(), CliError> {
        writeln!(io::stderr().lock(), "{message}")?;
        Ok(())
    }

    fn prompt(&self, label: &str, default: Option<&str>) -> Result<Option<String>, CliError> {
        let mut stderr = io::stderr().lock();
        match default {
            Some(default) => write!(stderr, "{label} [{default}]: ")?,
            None => write!(stderr, "{label}: ")?,
        }
        stderr.flush()?;
        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            return Ok(None);
        }
        let value = input.trim();
        Ok(Some(if value.is_empty() {
            default.unwrap_or_default().to_owned()
        } else {
            value.to_owned()
        }))
    }
}

pub(crate) struct LineTrackingOnboardingSession {
    prompter: Box<dyn TrackingSetupPrompter>,
}

impl LineTrackingOnboardingSession {
    pub(crate) fn terminal() -> Self {
        Self {
            prompter: Box::new(TerminalTrackingSetupPrompter),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_prompter(prompter: impl TrackingSetupPrompter + 'static) -> Self {
        Self {
            prompter: Box::new(prompter),
        }
    }

    fn required_prompt(
        &self,
        label: &str,
        default: Option<&str>,
    ) -> Result<Option<String>, CliError> {
        self.prompter.prompt(label, default)
    }

    fn confirm(&self, label: &str, default: bool) -> Result<Option<bool>, CliError> {
        let default_text = if default { "Y/n" } else { "y/N" };
        loop {
            let Some(value) = self.prompter.prompt(label, Some(default_text))? else {
                return Ok(None);
            };
            let value = value.trim();
            if value.eq_ignore_ascii_case("cancel") {
                return Ok(None);
            }
            if value.eq_ignore_ascii_case("y")
                || value.eq_ignore_ascii_case("yes")
                || (default && value.eq_ignore_ascii_case(default_text))
            {
                return Ok(Some(true));
            }
            if value.eq_ignore_ascii_case("n")
                || value.eq_ignore_ascii_case("no")
                || (!default && value.eq_ignore_ascii_case(default_text))
            {
                return Ok(Some(false));
            }
            self.prompter.message("Enter yes, no, or cancel.")?;
        }
    }

    fn collect_plan(&self) -> Result<Option<TrackingSetupPlan>, CliError> {
        self.prompter.message(PRIVACY_EXPLANATION)?;
        let current = std::env::current_dir()?;
        let Some(repositories) = self.required_prompt(
            "Git repository paths, comma-separated, or none",
            Some(current.to_string_lossy().as_ref()),
        )?
        else {
            return Ok(None);
        };
        let repos = parse_optional_paths(&repositories);

        let Some(ics) = self.required_prompt(
            "Optional calendar (.ics) paths, comma-separated",
            Some("none"),
        )?
        else {
            return Ok(None);
        };
        let ics_files = parse_optional_paths(&ics);
        test_local_sources(&repos, &ics_files)?;
        self.prompter
            .message("Selected local sources passed validation.")?;

        let Some(install_hooks) = self.confirm(
            "Install tracking-owned Claude Code hooks for future evidence?",
            false,
        )?
        else {
            return Ok(None);
        };
        let Some(at) = self.required_prompt("Weekday run time (HH:MM)", Some("18:45"))? else {
            return Ok(None);
        };
        let Some(schedule_timezone) = self.required_prompt("Schedule timezone", Some("local"))?
        else {
            return Ok(None);
        };
        let Some(install_scheduler) =
            self.confirm("Install tracking-owned weekday scheduler files?", false)?
        else {
            return Ok(None);
        };
        let scheduler_target = if install_scheduler {
            let default_target = default_scheduler_target().ok_or_else(|| {
                CliError::InvalidInput(
                    "automatic scheduler installation is not supported on this platform".to_owned(),
                )
            })?;
            let Some(target) = self.required_prompt(
                "Scheduler directory",
                Some(default_target.to_string_lossy().as_ref()),
            )?
            else {
                return Ok(None);
            };
            Some(PathBuf::from(target))
        } else {
            None
        };

        let (mode, authorize_automatic) = loop {
            let Some(mode) =
                self.required_prompt("Submission mode (draft, review, automatic)", Some("draft"))?
            else {
                return Ok(None);
            };
            let mode = match mode.to_ascii_lowercase().as_str() {
                "draft" => TrackingSubmissionMode::Draft,
                "review" => TrackingSubmissionMode::Review,
                "automatic" => TrackingSubmissionMode::Automatic,
                _ => {
                    self.prompter
                        .message("Choose draft, review, or automatic.")?;
                    continue;
                }
            };
            if mode != TrackingSubmissionMode::Automatic {
                break (mode, false);
            }
            let Some(authorized) = self.confirm(
                "Separately authorize automatic Tempo worklog submission?",
                false,
            )?
            else {
                return Ok(None);
            };
            if authorized {
                break (mode, true);
            }
            self.prompter.message(
                "Automatic mode was not authorized. Choose draft or review to continue without automatic submission.",
            )?;
        };

        Ok(Some(TrackingSetupPlan {
            mode,
            authorize_automatic,
            install_scheduler,
            install_hooks,
            scheduler_target,
            at,
            schedule_timezone,
            repos,
            ics_files,
        }))
    }
}

impl TrackingOnboardingSession for LineTrackingOnboardingSession {
    fn is_terminal(&self) -> bool {
        self.prompter.is_terminal()
    }

    fn run(&self) -> Result<TrackingOnboardingOutcome, CliError> {
        match self.confirm("Set up automatic tracking now?", false)? {
            Some(false) => Ok(TrackingOnboardingOutcome::Declined),
            None => Ok(TrackingOnboardingOutcome::Cancelled),
            Some(true) => self.collect_plan().map(|plan| {
                plan.map_or(
                    TrackingOnboardingOutcome::Cancelled,
                    TrackingOnboardingOutcome::Configure,
                )
            }),
        }
    }
}

fn parse_optional_paths(value: &str) -> Vec<PathBuf> {
    if value.trim().is_empty() || value.trim().eq_ignore_ascii_case("none") {
        return Vec::new();
    }
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn test_local_sources(repos: &[PathBuf], calendars: &[PathBuf]) -> Result<(), CliError> {
    for repo in repos {
        let healthy = repo.is_dir()
            && std::process::Command::new("git")
                .args([
                    "-C".as_ref(),
                    repo.as_os_str(),
                    "rev-parse".as_ref(),
                    "--git-dir".as_ref(),
                ])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
        if !healthy {
            return Err(CliError::InvalidInput(format!(
                "Git evidence source {} is not a repository",
                repo.display()
            )));
        }
    }
    for calendar in calendars {
        let body = std::fs::read_to_string(calendar).map_err(|error| {
            CliError::InvalidInput(format!(
                "cannot read calendar evidence source {}: {error}",
                calendar.display()
            ))
        })?;
        if !body.contains("BEGIN:VCALENDAR") || !body.contains("END:VCALENDAR") {
            return Err(CliError::InvalidInput(format!(
                "calendar evidence source {} is not a complete ICS calendar",
                calendar.display()
            )));
        }
    }
    Ok(())
}

fn default_scheduler_target() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        return dirs::config_dir().map(|path| path.join("systemd/user"));
    }
    #[cfg(target_os = "macos")]
    {
        return dirs::home_dir().map(|path| path.join("Library/LaunchAgents"));
    }
    #[allow(unreachable_code)]
    None
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use super::*;

    struct ScriptedPrompter {
        responses: Mutex<VecDeque<Option<String>>>,
        messages: Arc<Mutex<Vec<String>>>,
    }

    impl TrackingSetupPrompter for ScriptedPrompter {
        fn is_terminal(&self) -> bool {
            true
        }

        fn message(&self, message: &str) -> Result<(), CliError> {
            self.messages
                .lock()
                .map_err(|_| CliError::Api("test message lock poisoned".to_owned()))?
                .push(message.to_owned());
            Ok(())
        }

        fn prompt(&self, _label: &str, _default: Option<&str>) -> Result<Option<String>, CliError> {
            Ok(self
                .responses
                .lock()
                .map_err(|_| CliError::Api("test response lock poisoned".to_owned()))?
                .pop_front()
                .unwrap_or(None))
        }
    }

    #[test]
    fn shared_tracking_onboarding_explains_effects_and_keeps_authorizations_independent(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let calendar = directory.path().join("calendar.ics");
        std::fs::write(&calendar, "BEGIN:VCALENDAR\nVERSION:2.0\nEND:VCALENDAR\n")?;
        let messages = Arc::new(Mutex::new(Vec::new()));
        let session = LineTrackingOnboardingSession::with_prompter(ScriptedPrompter {
            responses: Mutex::new(VecDeque::from([
                Some("yes".to_owned()),
                Some("none".to_owned()),
                Some(calendar.to_string_lossy().into_owned()),
                Some("yes".to_owned()),
                Some("17:30".to_owned()),
                Some("Europe/Warsaw".to_owned()),
                Some("no".to_owned()),
                Some("review".to_owned()),
            ])),
            messages: Arc::clone(&messages),
        });

        let TrackingOnboardingOutcome::Configure(plan) = session.run()? else {
            return Err("tracking onboarding did not produce a plan".into());
        };

        assert!(plan.repos.is_empty());
        assert_eq!(plan.ics_files, [calendar]);
        assert!(plan.install_hooks);
        assert!(!plan.install_scheduler);
        assert_eq!(plan.at, "17:30");
        assert_eq!(plan.schedule_timezone, "Europe/Warsaw");
        assert_eq!(plan.mode, TrackingSubmissionMode::Review);
        assert!(!plan.authorize_automatic);
        let explanation = messages
            .lock()
            .map_err(|_| "test message lock poisoned")?
            .join(" ");
        for required in [
            "local evidence sources",
            "Raw evidence stays on this machine",
            "network boundaries",
            "Installing hooks",
            "allowing mutation",
            "passed validation",
        ] {
            assert!(explanation.contains(required), "missing {required}");
        }
        Ok(())
    }

    #[test]
    fn declining_automatic_submission_returns_to_safe_mode_selection(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let session = LineTrackingOnboardingSession::with_prompter(ScriptedPrompter {
            responses: Mutex::new(VecDeque::from([
                Some("yes".to_owned()),
                Some("none".to_owned()),
                Some("none".to_owned()),
                Some("no".to_owned()),
                Some("18:45".to_owned()),
                Some("local".to_owned()),
                Some("no".to_owned()),
                Some("automatic".to_owned()),
                Some("no".to_owned()),
                Some("draft".to_owned()),
            ])),
            messages,
        });

        let TrackingOnboardingOutcome::Configure(plan) = session.run()? else {
            return Err("tracking onboarding did not return a safe plan".into());
        };
        assert_eq!(plan.mode, TrackingSubmissionMode::Draft);
        assert!(!plan.authorize_automatic);
        Ok(())
    }

    #[test]
    fn end_of_input_cancels_before_any_plan_is_installed() -> Result<(), CliError> {
        let session = LineTrackingOnboardingSession::with_prompter(ScriptedPrompter {
            responses: Mutex::new(VecDeque::from([Some("yes".to_owned()), None])),
            messages: Arc::new(Mutex::new(Vec::new())),
        });
        assert_eq!(session.run()?, TrackingOnboardingOutcome::Cancelled);
        Ok(())
    }
}
