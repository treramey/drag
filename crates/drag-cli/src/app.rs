use std::env;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use drag::models::{Worklog, WorklogEntity};
use serde_json::json;

use crate::api::ApiClient;
use crate::cli::{
    AliasDeleteArgs, AliasSetArgs, DeleteArgs, DoctorArgs, ListArgs, LogArgs, SetupArgs,
};
#[cfg(test)]
use crate::config::{normalize_jira_site, JiraCredentials};
use crate::config::{Config, Credentials, TempoCredentials};
use crate::list::{self, ApiListDataSource};
use crate::log::{self, ApiLogGateway};
#[cfg(test)]
use crate::setup::LineOnboardingSession;
#[cfg(test)]
use crate::setup::{
    setup_cancelled, BrowserLauncher, ConnectionOutcome, NoopBrowserLauncher, OnboardingFuture,
    SecretInput, SetupPrompter, TerminalSetupPrompter, VerificationFuture, ATLASSIAN_TOKEN_URL,
};
use crate::setup::{
    ConnectionVerifier, OnboardingSession, OnboardingWorkflow, RemoteConnectionVerifier,
    SetupCredentials,
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
        }
    }

    pub async fn setup(&self, args: SetupArgs) -> Result<Rendered, CliError> {
        if args.from_env {
            // Validate before network requests; reload afterward to preserve concurrent updates.
            Config::load(&self.path)?;
            let setup_credentials = SetupCredentials::from_source(|name| {
                required_setup_environment(self.connection_environment.as_ref(), name)
            })?;
            return self
                .verify_and_save_environment_setup(setup_credentials)
                .await;
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
        setup_credentials: SetupCredentials,
    ) -> Result<Rendered, CliError> {
        let account_id = self
            .connection_verifier
            .verify_jira(&setup_credentials.jira_connection(), self.debug)
            .await?;
        let credentials = setup_credentials.to_credentials(account_id);
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

    pub async fn list(&self, args: ListArgs) -> Result<Rendered, CliError> {
        list::run(&self.path, self.now(), args, |credentials| {
            Ok(Box::new(ApiListDataSource::new(credentials, self.debug)?))
        })
        .await
    }

    pub async fn delete(&self, args: DeleteArgs) -> Result<Rendered, CliError> {
        let config = Config::load(&self.path)?;
        let credentials = config.credentials()?;
        let api = ApiClient::new(credentials, self.debug)?;
        let mut deleted = Vec::new();
        for id in args.worklog_ids {
            let entity = api.get_worklog(id).await?;
            let issue_key = api.get_issue_key(&entity.issue.id).await?;
            let worklog = self.to_worklog(entity, issue_key)?;
            if !args.dry_run {
                api.delete_worklog(id).await?;
            }
            deleted.push(worklog);
        }
        let human = deleted
            .iter()
            .map(|worklog| {
                if args.dry_run {
                    format!(
                        "Would delete worklog {} ({} {}).",
                        worklog.id, worklog.issue_key, worklog.duration
                    )
                } else {
                    format!(
                        "Deleted worklog {} ({} {}).",
                        worklog.id, worklog.issue_key, worklog.duration
                    )
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        Ok(Rendered::new(
            json!({"dryRun": args.dry_run, "worklogs": deleted}),
            human,
        ))
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

    fn to_worklog(&self, entity: WorklogEntity, issue_key: String) -> Result<Worklog, CliError> {
        log::to_worklog(entity, issue_key, self.timezone)
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
        Some(value) if !value.trim().is_empty() => Ok(value),
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
