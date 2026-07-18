use std::env;
use std::io::Write;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde_json::json;

use crate::cli::{
    AliasDeleteArgs, AliasSetArgs, DeleteArgs, DoctorArgs, ListArgs, LogArgs, SetupArgs,
};
#[cfg(test)]
use crate::config::{normalize_jira_site, JiraCredentials};
use crate::config::{Config, Credentials, TempoCredentials};
use crate::delete::{self, ApiDeleteGateway};
use crate::list::{self, ApiListDataSource};
use crate::list_tui::{ListReportSession, RatatuiListReportSession};
use crate::log::{self, ApiLogGateway};
#[cfg(test)]
use crate::setup::LineOnboardingSession;
#[cfg(test)]
use crate::setup::{
    setup_cancelled, BrowserLauncher, ConnectionOutcome, NoopBrowserLauncher, OnboardingFuture,
    SecretInput, SetupPrompter, TerminalSetupPrompter, VerificationFuture, ATLASSIAN_TOKEN_URL,
};
use crate::setup::{
    ConnectionVerifier, EnvironmentSetupPlan, OnboardingSession, OnboardingWorkflow,
    RemoteConnectionVerifier, SetupCredentials,
};
use crate::setup_tui::RatatuiOnboardingSession;
use crate::{CliError, Rendered};

pub struct App {
    path: PathBuf,
    timezone: Tz,
    debug: bool,
    connection_verifier: Box<dyn ConnectionVerifier>,
    connection_environment: Box<dyn ConnectionEnvironment>,
    onboarding_session: Box<dyn OnboardingSession>,
    list_report_session: Box<dyn ListReportSession>,
}

pub(crate) trait ConnectionEnvironment: Send + Sync {
    fn value(&self, name: &str) -> Option<String>;
    fn is_set(&self, name: &str) -> bool;
}

struct ProcessConnectionEnvironment;

impl ConnectionEnvironment for ProcessConnectionEnvironment {
    fn value(&self, name: &str) -> Option<String> {
        env::var(name).ok()
    }

    fn is_set(&self, name: &str) -> bool {
        env::var_os(name).is_some()
    }
}

#[cfg(test)]
struct EmptyConnectionEnvironment;

#[cfg(test)]
impl ConnectionEnvironment for EmptyConnectionEnvironment {
    fn value(&self, _name: &str) -> Option<String> {
        None
    }

    fn is_set(&self, _name: &str) -> bool {
        false
    }
}

impl App {
    pub fn new(path: PathBuf, timezone: Tz, debug: bool) -> Self {
        Self {
            path,
            timezone,
            debug,
            connection_verifier: Box::new(RemoteConnectionVerifier),
            connection_environment: Box::new(ProcessConnectionEnvironment),
            onboarding_session: Box::new(RatatuiOnboardingSession::terminal()),
            list_report_session: Box::new(RatatuiListReportSession::terminal()),
        }
    }

    #[cfg(test)]
    fn with_connection_verifier(
        path: PathBuf,
        connection_verifier: impl ConnectionVerifier + 'static,
    ) -> Self {
        Self {
            path,
            timezone: chrono_tz::UTC,
            debug: false,
            connection_verifier: Box::new(connection_verifier),
            connection_environment: Box::new(EmptyConnectionEnvironment),
            onboarding_session: Box::new(LineOnboardingSession::with_dependencies(
                TerminalSetupPrompter,
                NoopBrowserLauncher,
            )),
            list_report_session: Box::new(RatatuiListReportSession::terminal()),
        }
    }

    #[cfg(test)]
    fn with_setup_dependencies(
        path: PathBuf,
        connection_verifier: impl ConnectionVerifier + 'static,
        setup_prompter: impl SetupPrompter + 'static,
        browser_launcher: impl BrowserLauncher + 'static,
    ) -> Self {
        Self {
            path,
            timezone: chrono_tz::UTC,
            debug: false,
            connection_verifier: Box::new(connection_verifier),
            connection_environment: Box::new(EmptyConnectionEnvironment),
            onboarding_session: Box::new(LineOnboardingSession::with_dependencies(
                setup_prompter,
                browser_launcher,
            )),
            list_report_session: Box::new(RatatuiListReportSession::terminal()),
        }
    }

    #[cfg(test)]
    fn with_onboarding_session(
        path: PathBuf,
        connection_verifier: impl ConnectionVerifier + 'static,
        onboarding_session: impl OnboardingSession + 'static,
    ) -> Self {
        Self {
            path,
            timezone: chrono_tz::UTC,
            debug: false,
            connection_verifier: Box::new(connection_verifier),
            connection_environment: Box::new(EmptyConnectionEnvironment),
            onboarding_session: Box::new(onboarding_session),
            list_report_session: Box::new(RatatuiListReportSession::terminal()),
        }
    }

    #[cfg(test)]
    fn with_list_report_session(
        mut self,
        list_report_session: impl ListReportSession + 'static,
    ) -> Self {
        self.list_report_session = Box::new(list_report_session);
        self
    }

    pub async fn setup(&self, args: SetupArgs) -> Result<Rendered, CliError> {
        if args.from_env {
            // Validate before network requests; reload afterward to preserve concurrent updates.
            Config::load(&self.path)?;
            let setup_credentials = SetupCredentials::from_source(|name| {
                required_setup_environment(self.connection_environment.as_ref(), name)
            })?;
            let plan = EnvironmentSetupPlan::new(setup_credentials);
            if args.dry_run {
                return self.plan_environment_setup(plan, args.verify).await;
            }
            return self.verify_and_save_environment_setup(plan).await;
        }

        let config = Config::load(&self.path)?;
        if !self.onboarding_session.is_terminal() {
            return Err(CliError::InvalidInput(
                "interactive setup requires a terminal; use `drag setup --from-env` for automation"
                    .to_owned(),
            ));
        }
        self.run_interactive_setup(&config, !args.no_open).await
    }

    async fn plan_environment_setup(
        &self,
        plan: EnvironmentSetupPlan,
        verify: bool,
    ) -> Result<Rendered, CliError> {
        let verification = if verify {
            let account_id = self
                .connection_verifier
                .verify_jira(&plan.credentials().jira_connection(), self.debug)
                .await?;
            let credentials = plan.credentials().to_credentials(account_id);
            self.connection_verifier
                .verify_tempo(&TempoCredentials::from(&credentials), self.debug)
                .await?;
            json!({"status": "completed", "jira": "connected", "tempo": "connected"})
        } else {
            json!({"status": "planned", "jira": "read-only", "tempo": "read-only"})
        };

        Ok(Rendered::new(
            json!({
                "configured": false,
                "dryRun": true,
                "path": self.path,
                "source": "environment",
                "localValidation": {"status": "passed"},
                "remoteVerification": verification,
                "configuration": {
                    "status": "planned",
                    "credentials": "replace",
                    "aliases": "preserve"
                }
            }),
            format!(
                "Setup inputs are valid. Read-only verification is {} and configuration changes are planned; nothing was saved.",
                if verify { "complete" } else { "planned" }
            ),
        ))
    }

    pub async fn doctor(&self, args: DoctorArgs) -> Result<Rendered, CliError> {
        crate::doctor::run(
            &self.path,
            self.timezone,
            args.remote,
            self.debug,
            self.connection_environment.as_ref(),
            self.connection_verifier.as_ref(),
        )
        .await
    }

    async fn verify_and_save_environment_setup(
        &self,
        plan: EnvironmentSetupPlan,
    ) -> Result<Rendered, CliError> {
        let account_id = self
            .connection_verifier
            .verify_jira(&plan.credentials().jira_connection(), self.debug)
            .await?;
        let credentials = plan.credentials().to_credentials(account_id);
        self.connection_verifier
            .verify_tempo(&TempoCredentials::from(&credentials), self.debug)
            .await?;

        self.save_setup_credentials(credentials)?;

        Ok(Rendered::new(
            json!({
                "configured": true,
                "path": self.path,
                "source": "environment",
                "verification": {"jira": "connected", "tempo": "connected"}
            }),
            format!(
                "Verified Jira and Tempo using environment credentials. Configuration saved to {}.",
                self.path.display()
            ),
        ))
    }

    async fn run_interactive_setup(
        &self,
        existing: &Config,
        open_browser: bool,
    ) -> Result<Rendered, CliError> {
        let workflow = OnboardingWorkflow::new(
            existing,
            self.connection_verifier.as_ref(),
            self.debug,
            open_browser,
        );
        let credentials = self.onboarding_session.run(workflow).await?.finish()?;
        let data = json!({
            "configured": true,
            "path": self.path,
            "source": "interactive",
            "connection": {
                "jira": {"status": "connected", "hostname": credentials.hostname, "email": credentials.atlassian_user_email},
                "tempo": {"status": "connected"}
            }
        });
        let human = format!(
            "Connected {} to Jira and Tempo. Configuration saved to {}. Next, try `drag list`.",
            credentials.atlassian_user_email,
            self.path.display()
        );
        self.save_setup_credentials(credentials)?;
        Ok(Rendered::new(data, human))
    }

    fn save_setup_credentials(&self, credentials: Credentials) -> Result<(), CliError> {
        let mut config = Config::load(&self.path)?;
        config.tempo_token = Some(credentials.tempo_token);
        config.account_id = Some(credentials.account_id);
        config.atlassian_user_email = Some(credentials.atlassian_user_email);
        config.atlassian_token = Some(credentials.atlassian_token);
        config.hostname = Some(credentials.hostname);
        config.save(&self.path)
    }

    pub async fn log(&self, args: LogArgs) -> Result<Rendered, CliError> {
        log::run(&self.path, self.now(), args, |credentials| {
            ApiLogGateway::new(credentials, self.debug)
        })
        .await
    }

    pub async fn list(
        &self,
        args: ListArgs,
        interactive: bool,
    ) -> Result<Option<Rendered>, CliError> {
        let report = list::run_report(&self.path, self.now(), args, |credentials| {
            Ok(Box::new(ApiListDataSource::new(credentials, self.debug)?))
        })
        .await?;
        self.finish_list(report, interactive).await
    }

    async fn finish_list(
        &self,
        report: list::ListReport,
        interactive: bool,
    ) -> Result<Option<Rendered>, CliError> {
        if interactive && self.list_report_session.is_eligible() {
            self.list_report_session.run(&report).await?;
            Ok(None)
        } else {
            Ok(Some(report.rendered()))
        }
    }

    pub async fn list_stream(
        &self,
        args: ListArgs,
        writer: &mut impl Write,
    ) -> Result<(), CliError> {
        list::run_stream(
            &self.path,
            self.now(),
            args,
            |credentials| Ok(Box::new(ApiListDataSource::new(credentials, self.debug)?)),
            writer,
        )
        .await
    }

    pub async fn delete(&self, args: DeleteArgs) -> Result<Rendered, CliError> {
        delete::run(&self.path, self.timezone, args, |credentials| {
            ApiDeleteGateway::new(credentials, self.debug)
        })
        .await
    }

    pub fn alias_set(&self, args: AliasSetArgs) -> Result<Rendered, CliError> {
        crate::alias::set(&self.path, args)
    }

    pub fn alias_delete(&self, args: AliasDeleteArgs) -> Result<Rendered, CliError> {
        crate::alias::delete(&self.path, args)
    }

    pub fn alias_list(&self) -> Result<Rendered, CliError> {
        crate::alias::list(&self.path)
    }

    fn now(&self) -> DateTime<Tz> {
        Utc::now().with_timezone(&self.timezone)
    }
}

fn required_setup_environment(
    environment: &dyn ConnectionEnvironment,
    name: &str,
) -> Result<String, CliError> {
    match environment.value(name) {
        Some(value)
            if !value.trim().is_empty()
                && !value.chars().any(|character| character.is_control()) =>
        {
            Ok(value)
        }
        Some(value) if value.chars().any(|character| character.is_control()) => {
            Err(CliError::InvalidInput(format!(
                "{name} contains unsafe control characters for `drag setup --from-env`"
            )))
        }
        Some(_) => Err(CliError::InvalidInput(format!(
            "{name} must be set and non-empty for `drag setup --from-env`"
        ))),
        None if environment.is_set(name) => Err(CliError::InvalidInput(format!(
            "{name} must contain valid Unicode for `drag setup --from-env`"
        ))),
        None => Err(CliError::InvalidInput(format!(
            "{name} must be set and non-empty for `drag setup --from-env`"
        ))),
    }
}

pub fn default_timezone(explicit: Option<&str>) -> Result<Tz, CliError> {
    let name = explicit
        .map(str::to_owned)
        .unwrap_or_else(|| iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_owned()));
    name.parse()
        .map_err(|_| CliError::InvalidInput(format!("unknown IANA time zone: {name}")))
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
