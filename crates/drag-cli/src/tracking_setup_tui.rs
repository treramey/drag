//! Terminal consent for Claude Code activity capture during Drag setup.

use std::io::{self, IsTerminal, Write};

use crate::cli::TrackingSubmissionMode;
use crate::tracking_setup::{
    TrackingOnboardingOutcome, TrackingOnboardingSession, TrackingSetupPlan,
};
use crate::CliError;

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
            Some(value) => write!(stderr, "{label} [{value}]: ")?,
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
    fn confirm(&self) -> Result<Option<bool>, CliError> {
        loop {
            let Some(value) = self
                .prompter
                .prompt("Enable Claude Code activity capture?", Some("y/N"))?
            else {
                return Ok(None);
            };
            if value.eq_ignore_ascii_case("cancel") {
                return Ok(None);
            }
            if value.eq_ignore_ascii_case("y") || value.eq_ignore_ascii_case("yes") {
                return Ok(Some(true));
            }
            if value.eq_ignore_ascii_case("n")
                || value.eq_ignore_ascii_case("no")
                || value.eq_ignore_ascii_case("y/N")
            {
                return Ok(Some(false));
            }
            self.prompter.message("Answer yes or no.")?;
        }
    }
}
impl TrackingOnboardingSession for LineTrackingOnboardingSession {
    fn is_terminal(&self) -> bool {
        self.prompter.is_terminal()
    }
    fn run(&self) -> Result<TrackingOnboardingOutcome, CliError> {
        self.prompter.message("Claude Code tracking records minimized session start/end metadata locally. It does not contact Jira or Tempo and never creates worklogs during capture.")?;
        match self.confirm()? {
            Some(false) => Ok(TrackingOnboardingOutcome::Declined),
            None => Ok(TrackingOnboardingOutcome::Cancelled),
            Some(true) => Ok(TrackingOnboardingOutcome::Configure(TrackingSetupPlan {
                mode: TrackingSubmissionMode::Draft,
                authorize_automatic: false,
                install_scheduler: false,
                install_hooks: true,
                scheduler_target: None,
                at: "18:45".to_owned(),
                schedule_timezone: "local".to_owned(),
                repos: Vec::new(),
                ics_files: Vec::new(),
                clear_repos: true,
                clear_ics: true,
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
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
                .map_err(|_| CliError::Api("message lock poisoned".to_owned()))?
                .push(message.to_owned());
            Ok(())
        }
        fn prompt(&self, _: &str, _: Option<&str>) -> Result<Option<String>, CliError> {
            Ok(self
                .responses
                .lock()
                .map_err(|_| CliError::Api("response lock poisoned".to_owned()))?
                .pop_front()
                .unwrap_or(None))
        }
    }
    #[test]
    fn consent_creates_only_a_claude_capture_plan() -> Result<(), Box<dyn std::error::Error>> {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let session = LineTrackingOnboardingSession::with_prompter(ScriptedPrompter {
            responses: Mutex::new(VecDeque::from([Some("yes".to_owned())])),
            messages: Arc::clone(&messages),
        });
        let TrackingOnboardingOutcome::Configure(plan) = session.run()? else {
            return Err("missing plan".into());
        };
        assert!(plan.install_hooks);
        assert!(!plan.install_scheduler);
        assert!(!plan.authorize_automatic);
        let explanation = messages
            .lock()
            .map_err(|_| "message lock poisoned")?
            .join(" ");
        assert!(explanation.contains("minimized"));
        assert!(explanation.contains("never creates worklogs"));
        Ok(())
    }
    #[test]
    fn decline_and_end_of_input_have_no_plan() -> Result<(), CliError> {
        for (response, expected) in [
            (Some("no".to_owned()), TrackingOnboardingOutcome::Declined),
            (None, TrackingOnboardingOutcome::Cancelled),
        ] {
            let session = LineTrackingOnboardingSession::with_prompter(ScriptedPrompter {
                responses: Mutex::new(VecDeque::from([response])),
                messages: Arc::new(Mutex::new(Vec::new())),
            });
            assert_eq!(session.run()?, expected);
        }
        Ok(())
    }
}
