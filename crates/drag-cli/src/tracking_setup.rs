//! Shared interactive onboarding for automatic tracking.

use std::path::PathBuf;

use serde_json::Value;

use crate::cli::{TrackingSetupArgs, TrackingSubmissionMode};
use crate::tracking;
use crate::CliError;

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
    pub(crate) clear_repos: bool,
    pub(crate) clear_ics: bool,
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
            clear_repos: self.clear_repos,
            clear_ics: self.clear_ics,
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
    fn install(&self, plan: &TrackingSetupPlan) -> Result<Value, TrackingSetupInstallFailure>;
}

#[derive(Debug)]
pub(crate) struct TrackingSetupInstallFailure {
    pub(crate) error: CliError,
    pub(crate) recovery: Option<Value>,
}

impl From<CliError> for TrackingSetupInstallFailure {
    fn from(error: CliError) -> Self {
        Self {
            error,
            recovery: None,
        }
    }
}

impl<T: TrackingSetupInstaller + ?Sized> TrackingSetupInstaller for std::sync::Arc<T> {
    fn install(&self, plan: &TrackingSetupPlan) -> Result<Value, TrackingSetupInstallFailure> {
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
    fn install(&self, plan: &TrackingSetupPlan) -> Result<Value, TrackingSetupInstallFailure> {
        tracking::run_setup_capture(plan.clone().into_args(), &self.config_path)
    }
}
