use std::collections::BTreeMap;
use std::env;
use std::future::Future;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Days, Utc};
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
use crate::list_tui::{
    ListReportAction, ListReportSession, ListReportSuspenseOutcome, PendingListReportFuture,
    RatatuiListReportSession,
};
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

const LIST_FETCH_DEBOUNCE: Duration = Duration::from_millis(150);

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

struct AbortOnDropTask<T> {
    handle: tokio::task::JoinHandle<T>,
}

impl<T> AbortOnDropTask<T> {
    fn new(handle: tokio::task::JoinHandle<T>) -> Self {
        Self { handle }
    }

    fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    async fn join(mut self) -> Result<T, tokio::task::JoinError> {
        (&mut self.handle).await
    }
}

impl<T> Drop for AbortOnDropTask<T> {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

struct CachedListReport {
    report: list::ListReport,
    reusable: bool,
}

fn take_reusable_report(
    reports: &mut BTreeMap<chrono::NaiveDate, CachedListReport>,
    date: chrono::NaiveDate,
) -> Option<list::ListReport> {
    if !reports.get(&date).is_some_and(|cached| cached.reusable) {
        return None;
    }
    reports.remove(&date).map(|cached| cached.report)
}

fn date_for_list_action(
    date: chrono::NaiveDate,
    action: ListReportAction,
) -> Result<Option<chrono::NaiveDate>, CliError> {
    let date = match action {
        ListReportAction::Close => return Ok(None),
        ListReportAction::PreviousDate => date.checked_sub_days(Days::new(1)),
        ListReportAction::NextDate => date.checked_add_days(Days::new(1)),
    }
    .ok_or_else(|| CliError::InvalidInput("date is out of range".to_owned()))?;
    Ok(Some(date))
}

fn adjacent_dates(date: chrono::NaiveDate) -> Result<[chrono::NaiveDate; 2], CliError> {
    let previous = date
        .checked_sub_days(Days::new(1))
        .ok_or_else(|| CliError::InvalidInput("date is out of range".to_owned()))?;
    let next = date
        .checked_add_days(Days::new(1))
        .ok_or_else(|| CliError::InvalidInput("date is out of range".to_owned()))?;
    Ok([previous, next])
}

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
        mut args: ListArgs,
        interactive: bool,
    ) -> Result<Option<Rendered>, CliError> {
        if !interactive || !self.list_report_session.is_eligible() {
            let report = self.load_list_report(args).await?;
            return Ok(Some(report.rendered()));
        }

        let mut reports: BTreeMap<chrono::NaiveDate, CachedListReport> = BTreeMap::new();
        let mut prefetches: BTreeMap<
            chrono::NaiveDate,
            AbortOnDropTask<Result<list::ListReport, CliError>>,
        > = BTreeMap::new();
        let initial_report = self.load_list_report(args.clone()).await?;
        let mut requested_date = initial_report.selected_date();
        let mut displayed_date = requested_date;
        let mut initial_report = Some((initial_report, args.continue_from.is_none()));
        loop {
            let (report, reusable) = if let Some(initial_report) = initial_report.take() {
                initial_report
            } else if let Some(report) = take_reusable_report(&mut reports, requested_date) {
                (report, true)
            } else {
                let (ready, load): (bool, PendingListReportFuture<'_>) =
                    if let Some(task) = prefetches.remove(&requested_date) {
                        let ready = task.is_finished();
                        (ready, Box::pin(await_list_report_task(task)))
                    } else {
                        let load = self.load_list_report(args.clone());
                        (
                            false,
                            Box::pin(debounce_list_fetch(
                                tokio::time::sleep(LIST_FETCH_DEBOUNCE),
                                load,
                            )),
                        )
                    };
                let report = if ready {
                    load.await?
                } else {
                    let background = reports
                        .get(&displayed_date)
                        .map(|cached| &cached.report)
                        .ok_or_else(|| {
                            CliError::Io(std::io::Error::other(
                                "displayed list report was not cached",
                            ))
                        })?;
                    match self
                        .list_report_session
                        .suspense(requested_date, background, load)
                        .await?
                    {
                        ListReportSuspenseOutcome::Loaded(report) => *report,
                        ListReportSuspenseOutcome::Action(action) => {
                            let Some(next_date) = date_for_list_action(requested_date, action)?
                            else {
                                return Ok(None);
                            };
                            requested_date = next_date;
                            args.when = Some(requested_date.to_string());
                            args.continue_from = None;
                            continue;
                        }
                    }
                };
                (report, true)
            };
            let selected_date = report.selected_date();
            displayed_date = selected_date;

            for date in adjacent_dates(selected_date)? {
                if reports.contains_key(&date) || prefetches.contains_key(&date) {
                    continue;
                }
                let mut adjacent_args = args.clone();
                adjacent_args.when = Some(date.to_string());
                adjacent_args.continue_from = None;
                prefetches.insert(date, self.spawn_list_report(adjacent_args));
            }

            let action = self.list_report_session.run(&report).await?;
            let Some(next_date) = date_for_list_action(selected_date, action)? else {
                return Ok(None);
            };
            reports.insert(selected_date, CachedListReport { report, reusable });
            requested_date = next_date;
            args.when = Some(requested_date.to_string());
            args.continue_from = None;
        }
    }

    fn spawn_list_report(
        &self,
        args: ListArgs,
    ) -> AbortOnDropTask<Result<list::ListReport, CliError>> {
        let path = self.path.clone();
        let now = self.now();
        let debug = self.debug;
        AbortOnDropTask::new(tokio::spawn(async move {
            list::run_report(&path, now, args, |credentials| {
                Ok(Box::new(ApiListDataSource::new(credentials, debug)?))
            })
            .await
        }))
    }

    async fn load_list_report(&self, args: ListArgs) -> Result<list::ListReport, CliError> {
        list::run_report(&self.path, self.now(), args, |credentials| {
            Ok(Box::new(ApiListDataSource::new(credentials, self.debug)?))
        })
        .await
    }

    #[cfg(test)]
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

async fn await_list_report_task(
    task: AbortOnDropTask<Result<list::ListReport, CliError>>,
) -> Result<list::ListReport, CliError> {
    task.join().await.map_err(|error| {
        CliError::Io(std::io::Error::other(format!(
            "list prefetch failed: {error}"
        )))
    })?
}

async fn debounce_list_fetch<Q, F, T>(quiet_period: Q, load: F) -> T
where
    Q: Future<Output = ()>,
    F: Future<Output = T>,
{
    quiet_period.await;
    load.await
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
