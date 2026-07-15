use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::io::{self, Read};
use std::path::PathBuf;

use chrono::{DateTime, TimeZone, Utc};
use chrono_tz::Tz;
use comfy_table::{presets::UTF8_FULL, ContentArrangement, Table};
use drag::models::{AddWorklogRequest, Worklog, WorklogEntity};
use drag::schedule::{create_schedule_details, ScheduleDetails};
use drag::time::{
    clock_interval, format_duration, month_bounds, parse_clock, parse_duration_or_interval,
    select_date,
};
use drag::tracker::{Tracker, TrackerInterval};
use serde_json::json;
use url::Url;

use crate::api::ApiClient;
use crate::cli::{
    AliasDeleteArgs, AliasSetArgs, DeleteArgs, DoctorArgs, ListArgs, LogArgs, LogInput, SetupArgs,
    TrackerIssueArgs, TrackerStartArgs, TrackerStopArgs,
};
#[cfg(test)]
use crate::config::{normalize_jira_site, JiraCredentials};
use crate::config::{Config, Credentials, TempoCredentials};
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
use crate::{CliError, Rendered, EXIT_USAGE};

pub struct App {
    path: PathBuf,
    timezone: Tz,
    debug: bool,
    connection_verifier: Box<dyn ConnectionVerifier>,
    connection_environment: Box<dyn ConnectionEnvironment>,
    onboarding_session: Box<dyn OnboardingSession>,
}

trait ConnectionEnvironment: Send + Sync {
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

struct ServiceCheck {
    status: ServiceStatus,
    error_code: Option<&'static str>,
    exit_code: u8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ServiceStatus {
    Connected,
    NotConfigured,
    Failed,
}

impl ServiceStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::NotConfigured => "notConfigured",
            Self::Failed => "failed",
        }
    }
}

impl ServiceCheck {
    fn connected() -> Self {
        Self {
            status: ServiceStatus::Connected,
            error_code: None,
            exit_code: 0,
        }
    }

    fn not_configured() -> Self {
        Self {
            status: ServiceStatus::NotConfigured,
            error_code: None,
            exit_code: EXIT_USAGE,
        }
    }

    fn failed(error: &CliError) -> Self {
        Self {
            status: ServiceStatus::Failed,
            error_code: Some(error.code()),
            exit_code: error.exit_code(),
        }
    }

    fn preparation_failed(error: &CliError) -> Self {
        if matches!(error, CliError::NotConfigured(_)) {
            Self::not_configured()
        } else {
            Self::failed(error)
        }
    }

    fn is_connected(&self) -> bool {
        self.status == ServiceStatus::Connected
    }

    fn json(&self) -> serde_json::Value {
        let mut value = json!({"status": self.status.as_str()});
        if let Some(error_code) = self.error_code {
            value["errorCode"] = json!(error_code);
        }
        value
    }

    fn human(&self, service: &str) -> String {
        match self.status {
            ServiceStatus::Connected => format!("{service}: connected"),
            ServiceStatus::NotConfigured => format!("{service}: not configured"),
            ServiceStatus::Failed => format!(
                "{service}: failed ({})",
                self.error_code.unwrap_or("runtime_failure")
            ),
        }
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
        let config = Config::load(&self.path)?;
        let configured = configured_fields(&config, self.connection_environment.as_ref());
        let jira_configured = configured["atlassianHost"].as_bool() == Some(true)
            && configured["atlassianEmail"].as_bool() == Some(true)
            && configured["atlassianToken"].as_bool() == Some(true);
        let tempo_configured = configured["tempoToken"].as_bool() == Some(true)
            && configured["accountId"].as_bool() == Some(true);
        let mut report = json!({
            "name": "drag",
            "version": env!("CARGO_PKG_VERSION"),
            "configPath": self.path,
            "configured": configured,
            "aliases": config.aliases.len(),
            "trackers": config.trackers.len(),
            "timezone": self.timezone.name(),
            "target": {
                "architecture": std::env::consts::ARCH,
                "operatingSystem": std::env::consts::OS
            }
        });
        let mut human = format!(
            "drag {}\nconfig: {}\ntimezone: {}\naliases: {}\ntrackers: {}\nJira: {}\nTempo: {}",
            env!("CARGO_PKG_VERSION"),
            self.path.display(),
            self.timezone.name(),
            config.aliases.len(),
            config.trackers.len(),
            configured_label(jira_configured),
            configured_label(tempo_configured),
        );

        if !args.remote {
            return Ok(Rendered::new(report, human));
        }

        let jira = match config
            .jira_credentials_from_source(|name| self.connection_environment.value(name))
        {
            Ok(connection) => match self
                .connection_verifier
                .verify_jira(&connection, self.debug)
                .await
            {
                Ok(_) => ServiceCheck::connected(),
                Err(error) => ServiceCheck::failed(&error),
            },
            Err(error) => ServiceCheck::preparation_failed(&error),
        };
        let tempo = match config
            .tempo_credentials_from_source(|name| self.connection_environment.value(name))
        {
            Ok(connection) => match self
                .connection_verifier
                .verify_tempo(&connection, self.debug)
                .await
            {
                Ok(()) => ServiceCheck::connected(),
                Err(error) => ServiceCheck::failed(&error),
            },
            Err(error) => ServiceCheck::preparation_failed(&error),
        };
        let successful = jira.is_connected() && tempo.is_connected();
        let failure_exit_code = jira.exit_code.max(tempo.exit_code);
        report["remoteChecks"] = json!({
            "jira": jira.json(),
            "tempo": tempo.json(),
        });
        human.push_str(&format!(
            "\n\nRemote checks (read-only)\n{}\n{}",
            jira.human("Jira"),
            tempo.human("Tempo")
        ));

        if successful {
            Ok(Rendered::new(report, human))
        } else {
            Ok(Rendered::failed(
                report,
                human,
                "remote_check_failed",
                "one or more remote connection checks failed",
                failure_exit_code,
            ))
        }
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
        let input = log_input(args)?;
        let config = Config::load(&self.path)?;
        let credentials = config.credentials()?;
        let mut request = self.build_add_request(&config, &credentials, &input)?;
        let issue_key = config
            .resolve_issue(&input.value.issue_key_or_alias)
            .to_uppercase();
        if input.dry_run {
            return Ok(Rendered::new(
                json!({"dryRun": true, "issueKey": issue_key, "request": request}),
                format!(
                    "Would log {} to {}.",
                    format_duration(request.time_spent_seconds, false),
                    input.value.issue_key_or_alias
                ),
            ));
        }
        let api = ApiClient::new(credentials, self.debug)?;
        request.issue_id = api.get_issue_id(&issue_key).await?;
        let entity = api.add_worklog(request).await?;
        let worklog = self.to_worklog(entity, issue_key)?;
        Ok(Rendered::new(
            serde_json::to_value(&worklog)?,
            format!(
                "Successfully logged {} to {}, type `drag d {}` to undo.",
                worklog.duration, worklog.issue_key, worklog.id
            ),
        ))
    }

    pub async fn list(&self, args: ListArgs) -> Result<Rendered, CliError> {
        let config = Config::load(&self.path)?;
        let credentials = config.credentials()?;
        let now = self.now();
        let selected = select_date(now, args.when.as_deref())?;
        let (month_start, month_end) = month_bounds(selected.date);
        let month_start = month_start.to_string();
        let month_end = month_end.to_string();
        let api = ApiClient::new(credentials.clone(), self.debug)?;
        let (entities, schedule) = tokio::try_join!(
            api.get_worklogs(&month_start, &month_end),
            api.get_schedule(&month_start, &month_end)
        )?;
        let details = create_schedule_details(
            &entities,
            &schedule,
            selected.date,
            now.date_naive(),
            &credentials.account_id,
        );
        let issue_ids: BTreeSet<_> = entities
            .iter()
            .map(|entity| entity.issue.id.clone())
            .collect();
        let mut issue_keys = BTreeMap::new();
        for issue_id in issue_ids {
            issue_keys.insert(issue_id.clone(), api.get_issue_key(&issue_id).await?);
        }
        let mut worklogs = Vec::new();
        for entity in entities.iter().filter(|entity| {
            entity.author.account_id == credentials.account_id
                && entity.start_date == selected.date.to_string()
        }) {
            let issue_key = issue_keys
                .get(&entity.issue.id)
                .cloned()
                .ok_or_else(|| CliError::Api("Atlassian did not return an issue key".to_owned()))?;
            worklogs.push(self.to_worklog(entity.clone(), issue_key)?);
        }
        let human = worklogs_table(
            selected.date,
            &worklogs,
            &details,
            args.verbose,
            &config.aliases,
        );
        Ok(Rendered::new(
            json!({"date": selected.date, "worklogs": worklogs, "schedule": details}),
            human,
        ))
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
        let mut config = Config::load(&self.path)?;
        config
            .aliases
            .insert(args.alias.clone(), args.issue_key.clone());
        config.save(&self.path)?;
        Ok(Rendered::new(
            json!({"alias": args.alias, "issueKey": args.issue_key}),
            format!("{} => {}", args.alias, args.issue_key),
        ))
    }

    pub fn alias_delete(&self, args: AliasDeleteArgs) -> Result<Rendered, CliError> {
        let mut config = Config::load(&self.path)?;
        let issue_key = config.aliases.remove(&args.alias_name);
        config.save(&self.path)?;
        Ok(Rendered::new(
            json!({"alias": args.alias_name, "deleted": issue_key.is_some(), "issueKey": issue_key}),
            if issue_key.is_some() {
                format!("Deleted alias {}.", args.alias_name)
            } else {
                format!("Alias {} did not exist.", args.alias_name)
            },
        ))
    }

    pub fn alias_list(&self) -> Result<Rendered, CliError> {
        let config = Config::load(&self.path)?;
        let human = if config.aliases.is_empty() {
            "No aliases configured.".to_owned()
        } else {
            config
                .aliases
                .iter()
                .map(|(alias, issue)| format!("{alias} => {issue}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok(Rendered::new(json!({"aliases": config.aliases}), human))
    }

    pub async fn tracker_start(&self, args: TrackerStartArgs) -> Result<Rendered, CliError> {
        let mut config = Config::load(&self.path)?;
        let issue_key = config.resolve_issue(&args.issue_key_or_alias);
        if config.trackers.contains_key(&issue_key) && args.stop_previous {
            drop(config);
            self.tracker_stop(TrackerStopArgs {
                issue_key_or_alias: args.issue_key_or_alias.clone(),
                description: None,
                remaining_estimate: None,
                dry_run: false,
            })
            .await?;
            config = Config::load(&self.path)?;
        }
        if config.trackers.contains_key(&issue_key) {
            return Err(CliError::InvalidInput(format!(
                "tracker for {} already exists",
                args.issue_key_or_alias
            )));
        }
        let tracker = Tracker::new(issue_key.clone(), args.description, self.now_millis());
        config.trackers.insert(issue_key.clone(), tracker.clone());
        config.save(&self.path)?;
        Ok(Rendered::new(
            serde_json::to_value(&tracker)?,
            format!("Started tracker for {issue_key}."),
        ))
    }

    pub fn tracker_pause(&self, args: TrackerIssueArgs) -> Result<Rendered, CliError> {
        self.update_tracker(args, |tracker, now| tracker.pause(now), "Paused")
    }

    pub fn tracker_resume(&self, args: TrackerIssueArgs) -> Result<Rendered, CliError> {
        self.update_tracker(args, |tracker, now| tracker.resume(now), "Resumed")
    }

    pub fn tracker_delete(&self, args: TrackerIssueArgs) -> Result<Rendered, CliError> {
        let mut config = Config::load(&self.path)?;
        let issue_key = config.resolve_issue(&args.issue_key_or_alias);
        let tracker = config.trackers.remove(&issue_key).ok_or_else(|| {
            CliError::InvalidInput(format!(
                "tracker for {} does not exist",
                args.issue_key_or_alias
            ))
        })?;
        config.save(&self.path)?;
        Ok(Rendered::new(
            serde_json::to_value(&tracker)?,
            format!("Deleted tracker for {issue_key}."),
        ))
    }

    pub fn tracker_list(&self) -> Result<Rendered, CliError> {
        let config = Config::load(&self.path)?;
        let now = self.now_millis();
        let human = trackers_table(&config.trackers, &config.aliases, now, self.timezone);
        Ok(Rendered::new(
            json!({"trackers": config.trackers.values().map(|tracker| json!({
                "tracker": tracker,
                "totalMinutes": tracker.total_minutes(now)
            })).collect::<Vec<_>>() }),
            human,
        ))
    }

    pub async fn tracker_stop(&self, args: TrackerStopArgs) -> Result<Rendered, CliError> {
        let mut config = Config::load(&self.path)?;
        let issue_key = config.resolve_issue(&args.issue_key_or_alias);
        let now = self.now_millis();
        let mut tracker = config.trackers.get(&issue_key).cloned().ok_or_else(|| {
            CliError::InvalidInput(format!(
                "tracker for {} does not exist",
                args.issue_key_or_alias
            ))
        })?;
        tracker.stop(now, args.description);
        let requests =
            self.tracker_requests(&config, &tracker, args.remaining_estimate.as_deref())?;
        if args.dry_run {
            return Ok(Rendered::new(
                json!({"dryRun": true, "requests": requests}),
                format!(
                    "Would upload {} interval(s) for {issue_key}.",
                    requests.len()
                ),
            ));
        }
        // Persist the stopped tracker before the first network request. If the
        // process or API fails, every not-yet-uploaded interval remains local.
        config.trackers.insert(issue_key.clone(), tracker.clone());
        config.save(&self.path)?;
        if requests.is_empty() {
            config.trackers.remove(&issue_key);
            config.save(&self.path)?;
            return Ok(Rendered::new(
                json!({"issueKey": issue_key, "worklogs": []}),
                "Tracker had no intervals of at least one minute; it was removed.".to_owned(),
            ));
        }

        let credentials = config.credentials()?;
        let api = ApiClient::new(credentials, self.debug)?;
        let issue_id = api.get_issue_id(&tracker.issue_key.to_uppercase()).await?;
        let mut uploaded = Vec::new();
        let mut failures = Vec::new();
        for (interval, mut request) in requests {
            request.issue_id.clone_from(&issue_id);
            match api.add_worklog(request).await {
                Ok(entity) => {
                    uploaded.push(entity.tempo_worklog_id);
                    tracker.intervals.retain(|candidate| candidate != &interval);
                    config.trackers.insert(issue_key.clone(), tracker.clone());
                    config.save(&self.path)?;
                }
                Err(error) => failures.push(error.to_string()),
            }
        }
        if failures.is_empty() {
            config.trackers.remove(&issue_key);
            config.save(&self.path)?;
            Ok(Rendered::new(
                json!({"issueKey": issue_key, "uploadedWorklogIds": uploaded}),
                format!("Logged all tracker intervals for {issue_key}."),
            ))
        } else {
            Err(CliError::Api(format!(
                "failed to upload {} interval(s); successful intervals were removed from the tracker: {}",
                failures.len(),
                failures.join("; ")
            )))
        }
    }

    fn update_tracker(
        &self,
        args: TrackerIssueArgs,
        action: impl FnOnce(&mut Tracker, i64),
        verb: &str,
    ) -> Result<Rendered, CliError> {
        let mut config = Config::load(&self.path)?;
        let issue_key = config.resolve_issue(&args.issue_key_or_alias);
        let tracker = config.trackers.get_mut(&issue_key).ok_or_else(|| {
            CliError::InvalidInput(format!(
                "tracker for {} does not exist",
                args.issue_key_or_alias
            ))
        })?;
        action(tracker, self.now_millis());
        let value = serde_json::to_value(&*tracker)?;
        config.save(&self.path)?;
        Ok(Rendered::new(
            value,
            format!("{verb} tracker for {issue_key}."),
        ))
    }

    fn build_add_request(
        &self,
        config: &Config,
        credentials: &Credentials,
        input: &ResolvedLogInput,
    ) -> Result<AddWorklogRequest, CliError> {
        let selected = select_date(self.now(), input.value.when.as_deref())?;
        let parsed = parse_duration_or_interval(
            &input.value.duration_or_interval,
            selected.date,
            self.timezone,
        )?;
        if parsed.seconds <= 0 {
            return Err(drag::Error::NonPositiveDuration.into());
        }
        let start = if let Some(start) = parsed.start_time {
            start
        } else if let Some(start) = &input.value.start {
            parse_clock(start).ok_or_else(|| drag::Error::InvalidTime(start.clone()))?
        } else {
            selected.default_start_time
        };
        let remaining_estimate_seconds = input
            .value
            .remaining_estimate
            .as_deref()
            .map(|remaining| {
                parse_duration_or_interval(remaining, selected.date, self.timezone)
                    .map(|parsed| parsed.seconds)
            })
            .transpose()?;
        let issue_key = config
            .resolve_issue(&input.value.issue_key_or_alias)
            .to_uppercase();
        // The issue ID is filled by the async caller; this marker is replaced before upload.
        Ok(AddWorklogRequest {
            issue_id: format!("<resolved from {issue_key}>"),
            time_spent_seconds: parsed.seconds,
            start_date: selected.date.to_string(),
            start_time: start.format("%H:%M:%S").to_string(),
            description: input.value.description.clone(),
            remaining_estimate_seconds,
            author_account_id: Some(credentials.account_id.clone()),
        })
    }

    fn tracker_requests(
        &self,
        config: &Config,
        tracker: &Tracker,
        remaining: Option<&str>,
    ) -> Result<Vec<(TrackerInterval, AddWorklogRequest)>, CliError> {
        let credentials = config.credentials()?;
        tracker
            .intervals
            .iter()
            .map(|interval| {
                let start = self
                    .timezone
                    .timestamp_millis_opt(interval.start)
                    .single()
                    .ok_or_else(|| {
                        CliError::InvalidInput("tracker has an invalid timestamp".to_owned())
                    })?;
                let minutes = (interval.end - interval.start) / 60_000;
                let input = ResolvedLogInput {
                    value: LogInput {
                        issue_key_or_alias: tracker.issue_key.clone(),
                        duration_or_interval: format!("{minutes}m"),
                        when: Some(start.date_naive().to_string()),
                        description: tracker.description.clone(),
                        start: Some(start.format("%H:%M").to_string()),
                        remaining_estimate: remaining.map(str::to_owned),
                    },
                    dry_run: false,
                };
                Ok((
                    *interval,
                    self.build_add_request(config, &credentials, &input)?,
                ))
            })
            .collect()
    }

    fn to_worklog(&self, entity: WorklogEntity, issue_key: String) -> Result<Worklog, CliError> {
        let date = chrono::NaiveDate::parse_from_str(&entity.start_date, "%Y-%m-%d")
            .map_err(|_| CliError::Api("Tempo returned an invalid start date".to_owned()))?;
        let hostname = Url::parse(&entity.issue.self_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .ok_or_else(|| CliError::Api("Tempo returned an invalid issue URL".to_owned()))?;
        Ok(Worklog {
            id: entity.tempo_worklog_id,
            interval: clock_interval(
                entity.time_spent_seconds,
                &entity.start_time,
                date,
                self.timezone,
            ),
            issue_id: entity.issue.id,
            duration: format_duration(entity.time_spent_seconds, false),
            description: entity.description,
            link: format!("https://{hostname}/browse/{issue_key}"),
            issue_key,
        })
    }

    fn now(&self) -> DateTime<Tz> {
        Utc::now().with_timezone(&self.timezone)
    }

    fn now_millis(&self) -> i64 {
        Utc::now().timestamp_millis()
    }
}

fn configured_label(configured: bool) -> &'static str {
    if configured {
        "configured"
    } else {
        "not configured"
    }
}

fn configured_fields(
    config: &Config,
    connection_environment: &dyn ConnectionEnvironment,
) -> serde_json::Value {
    json!({
        "tempoToken": config.tempo_token.is_some() || connection_environment.is_set("TEMPO_TOKEN"),
        "accountId": config.account_id.is_some() || connection_environment.is_set("TEMPO_ACCOUNT_ID"),
        "atlassianEmail": config.atlassian_user_email.is_some() || connection_environment.is_set("ATLASSIAN_EMAIL"),
        "atlassianToken": config.atlassian_token.is_some() || connection_environment.is_set("ATLASSIAN_TOKEN"),
        "atlassianHost": config.hostname.is_some() || connection_environment.is_set("ATLASSIAN_HOST"),
    })
}

struct ResolvedLogInput {
    value: LogInput,
    dry_run: bool,
}

fn log_input(args: LogArgs) -> Result<ResolvedLogInput, CliError> {
    let value = if let Some(raw) = args.json {
        let raw = if raw == "-" {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            input
        } else {
            raw
        };
        serde_json::from_str(&raw)?
    } else {
        LogInput {
            issue_key_or_alias: args
                .issue_key_or_alias
                .ok_or_else(|| CliError::InvalidInput("missing issue key or alias".to_owned()))?,
            duration_or_interval: args
                .duration_or_interval
                .ok_or_else(|| CliError::InvalidInput("missing duration or interval".to_owned()))?,
            when: args.when,
            description: args.description,
            start: args.start,
            remaining_estimate: args.remaining_estimate,
        }
    };
    Ok(ResolvedLogInput {
        value,
        dry_run: args.dry_run,
    })
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

fn worklogs_table(
    date: chrono::NaiveDate,
    worklogs: &[Worklog],
    details: &ScheduleDetails,
    verbose: bool,
    aliases: &BTreeMap<String, String>,
) -> String {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic);
    let mut header = vec!["id", "from-to", "issue", "duration"];
    if verbose {
        header.extend(["description", "issue url"]);
    }
    table.set_header(header);
    for worklog in worklogs {
        let interval = worklog.interval.as_ref().map_or_else(
            || "unknown".to_owned(),
            |value| format!("{}-{}", value.start_time, value.end_time),
        );
        let mut row = vec![
            worklog.id.clone(),
            interval,
            issue_with_aliases(&worklog.issue_key, aliases, true),
            worklog.duration.clone(),
        ];
        if verbose {
            row.extend([worklog.description.clone(), worklog.link.clone()]);
        }
        table.add_row(row);
    }
    format!(
        "{}: {}/{} ({})\n{}\n{}\nRequired {}, logged: {}",
        date.format("%B"),
        details.month_logged_duration,
        details.month_required_duration,
        details.month_current_period_duration,
        date.format("%A, %Y-%m-%d"),
        if worklogs.is_empty() {
            "No worklogs".to_owned()
        } else {
            table.to_string()
        },
        details.day_required_duration,
        details.day_logged_duration
    )
}

fn trackers_table(
    trackers: &BTreeMap<String, Tracker>,
    aliases: &BTreeMap<String, String>,
    now: i64,
    timezone: Tz,
) -> String {
    if trackers.is_empty() {
        return "No trackers.".to_owned();
    }
    trackers
        .values()
        .map(|tracker| {
            let state = if tracker.is_active {
                "Active"
            } else {
                "INACTIVE"
            };
            let resumed = timezone
                .timestamp_millis_opt(tracker.active_timestamp)
                .single()
                .map_or_else(
                    || "invalid timestamp".to_owned(),
                    |value| value.format("%Y-%m-%d %H:%M").to_string(),
                );
            let intervals = if tracker.intervals.is_empty() {
                "No completed intervals".to_owned()
            } else {
                tracker
                    .intervals
                    .iter()
                    .map(|interval| {
                        let start = timezone.timestamp_millis_opt(interval.start).single();
                        let end = timezone.timestamp_millis_opt(interval.end).single();
                        match (start, end) {
                            (Some(start), Some(end)) => format!(
                                "{} - {} ({}m)",
                                start.format("%Y-%m-%d %H:%M:%S"),
                                end.format("%Y-%m-%d %H:%M:%S"),
                                (interval.end - interval.start) / 60_000
                            ),
                            _ => "invalid interval".to_owned(),
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            format!(
                "Tracker for {}, {state}\nLast resume time: {resumed}\n{intervals}\nTotal duration: {}m{}",
                issue_with_aliases(&tracker.issue_key, aliases, false),
                tracker.total_minutes(now),
                tracker
                    .description
                    .as_ref()
                    .map_or_else(String::new, |value| format!("\n{value}"))
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn issue_with_aliases(
    issue_key: &str,
    aliases: &BTreeMap<String, String>,
    aliases_first: bool,
) -> String {
    let names: Vec<_> = aliases
        .iter()
        .filter(|(_, issue)| issue.eq_ignore_ascii_case(issue_key))
        .map(|(alias, _)| alias.as_str())
        .collect();
    let Some(first) = names.first() else {
        return issue_key.to_owned();
    };
    let truncated = if first.chars().count() > 17 {
        format!("{}…", first.chars().take(16).collect::<String>())
    } else {
        (*first).to_owned()
    };
    let suffix = if names.len() > 1 {
        format!(", +{}", names.len() - 1)
    } else {
        String::new()
    };
    let label = format!("({truncated}{suffix})");
    if aliases_first {
        format!("{label} {issue_key}")
    } else {
        format!("{issue_key} {label}")
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
mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    #[cfg(unix)]
    use std::process::Command;
    #[cfg(unix)]
    use std::time::Duration;

    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use drag::tracker::Tracker;
    #[cfg(unix)]
    use expectrl::session::OsSession;
    #[cfg(unix)]
    use expectrl::{ControlCode, Eof, Expect, Session};
    use tempfile::TempDir;

    use super::{
        normalize_jira_site, App, BrowserLauncher, Config, ConnectionOutcome, ConnectionVerifier,
        JiraCredentials, NoopBrowserLauncher, OnboardingFuture, OnboardingSession,
        OnboardingWorkflow, RatatuiOnboardingSession, SecretInput, SetupCredentials, SetupPrompter,
        TempoCredentials, VerificationFuture, ATLASSIAN_TOKEN_URL,
    };
    use crate::cli::{DoctorArgs, SetupArgs};
    use crate::CliError;
    #[cfg(unix)]
    use crate::ResolvedOutputMode;

    struct FakeVerifier {
        jira_error: Option<String>,
        tempo_error: Option<String>,
        tempo_accounts: Arc<Mutex<Vec<String>>>,
        config_update: Option<(PathBuf, Config)>,
    }

    #[derive(Default)]
    struct PromptState {
        text_responses: VecDeque<String>,
        secret_responses: VecDeque<Option<String>>,
        text_prompts: Vec<(String, Option<String>)>,
        secret_prompts: Vec<(String, bool)>,
        messages: Vec<String>,
        browser_urls: Vec<String>,
        browser_failure: Option<String>,
        events: Vec<String>,
    }

    struct FakePrompter {
        terminal: bool,
        state: Arc<Mutex<PromptState>>,
    }

    struct FakeBrowserLauncher {
        state: Arc<Mutex<PromptState>>,
    }

    struct FakeConnectionEnvironment {
        values: BTreeMap<String, String>,
    }

    impl super::ConnectionEnvironment for FakeConnectionEnvironment {
        fn value(&self, name: &str) -> Option<String> {
            self.values.get(name).cloned()
        }

        fn is_set(&self, name: &str) -> bool {
            self.values.contains_key(name)
        }
    }

    impl BrowserLauncher for FakeBrowserLauncher {
        fn open(&self, url: &url::Url) -> std::io::Result<()> {
            let mut state = self
                .state
                .lock()
                .map_err(|_| std::io::Error::other("test browser lock poisoned"))?;
            state.browser_urls.push(url.as_str().to_owned());
            state.events.push(format!("browser:{url}"));
            match &state.browser_failure {
                Some(message) => Err(std::io::Error::other(message.clone())),
                None => Ok(()),
            }
        }
    }

    impl SetupPrompter for FakePrompter {
        fn is_terminal(&self) -> bool {
            self.terminal
        }

        fn message(&self, message: &str) -> Result<(), CliError> {
            let mut state = self
                .state
                .lock()
                .map_err(|_| CliError::Io(std::io::Error::other("test prompt lock poisoned")))?;
            state.messages.push(message.to_owned());
            state.events.push(format!("message:{message}"));
            Ok(())
        }

        fn prompt_text(&self, label: &str, default: Option<&str>) -> Result<String, CliError> {
            let mut state = self
                .state
                .lock()
                .map_err(|_| CliError::Io(std::io::Error::other("test prompt lock poisoned")))?;
            state
                .text_prompts
                .push((label.to_owned(), default.map(str::to_owned)));
            let response = state
                .text_responses
                .pop_front()
                .ok_or_else(super::setup_cancelled)?;
            if response.is_empty() {
                Ok(default.unwrap_or_default().to_owned())
            } else {
                Ok(response)
            }
        }

        fn prompt_secret(&self, label: &str, can_retain: bool) -> Result<Option<String>, CliError> {
            let mut state = self
                .state
                .lock()
                .map_err(|_| CliError::Io(std::io::Error::other("test prompt lock poisoned")))?;
            state.secret_prompts.push((label.to_owned(), can_retain));
            state.events.push(format!("secret:{label}"));
            state
                .secret_responses
                .pop_front()
                .ok_or_else(super::setup_cancelled)
        }
    }

    struct SequenceVerifier {
        jira_results: Mutex<VecDeque<Result<String, VerificationFailure>>>,
        tempo_results: Mutex<VecDeque<Result<(), VerificationFailure>>>,
    }

    struct ScriptedOnboardingSession {
        events: Arc<Mutex<Vec<String>>>,
    }

    struct IncompleteOnboardingSession;

    struct PendingJiraVerifier;

    struct PendingTempoVerifier;

    struct DoctorVerifier {
        jira_result: Mutex<Option<Result<String, VerificationFailure>>>,
        tempo_result: Mutex<Option<Result<(), VerificationFailure>>>,
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    enum VerificationFailure {
        Authentication(String),
        Fatal(String),
    }

    impl OnboardingSession for ScriptedOnboardingSession {
        fn is_terminal(&self) -> bool {
            true
        }

        fn run<'a>(&'a self, mut workflow: OnboardingWorkflow<'a>) -> OnboardingFuture<'a> {
            let events = Arc::clone(&self.events);
            Box::pin(async move {
                let jira_page = workflow.jira_token_page()?;
                events
                    .lock()
                    .map_err(|_| CliError::Api("test session lock was poisoned".to_owned()))?
                    .push(format!("jira-browser:{}", jira_page.open_browser));
                match workflow
                    .connect_jira(
                        "https://Example.atlassian.net/jira/software".to_owned(),
                        " scripted@example.com ".to_owned(),
                        SecretInput::Replace("scripted-jira-token".to_owned()),
                    )
                    .await?
                {
                    ConnectionOutcome::Connected => {}
                    ConnectionOutcome::Rejected(error) => return Err(error),
                }

                let tempo_page = workflow.tempo_token_page()?;
                events
                    .lock()
                    .map_err(|_| CliError::Api("test session lock was poisoned".to_owned()))?
                    .push(format!("tempo-browser:{}", tempo_page.open_browser));
                match workflow
                    .connect_tempo(SecretInput::Replace("scripted-tempo-token".to_owned()))
                    .await?
                {
                    ConnectionOutcome::Connected => {}
                    ConnectionOutcome::Rejected(error) => return Err(error),
                }

                events
                    .lock()
                    .map_err(|_| CliError::Api("test session lock was poisoned".to_owned()))?
                    .push("save".to_owned());
                Ok(workflow)
            })
        }
    }

    impl OnboardingSession for IncompleteOnboardingSession {
        fn is_terminal(&self) -> bool {
            true
        }

        fn run<'a>(&'a self, workflow: OnboardingWorkflow<'a>) -> OnboardingFuture<'a> {
            Box::pin(async move { Ok(workflow) })
        }
    }

    impl VerificationFailure {
        fn into_cli_error(self) -> CliError {
            match self {
                Self::Authentication(message) => CliError::Authentication(message),
                Self::Fatal(message) => CliError::Api(message),
            }
        }
    }

    impl ConnectionVerifier for SequenceVerifier {
        fn verify_jira<'a>(
            &'a self,
            _connection: &'a JiraCredentials,
            _debug: bool,
        ) -> VerificationFuture<'a, String> {
            Box::pin(async move {
                self.jira_results
                    .lock()
                    .map_err(|_| CliError::Api("test verifier lock was poisoned".to_owned()))?
                    .pop_front()
                    .ok_or_else(|| CliError::Api("unexpected Jira verification".to_owned()))?
                    .map_err(VerificationFailure::into_cli_error)
            })
        }

        fn verify_tempo<'a>(
            &'a self,
            _connection: &'a TempoCredentials,
            _debug: bool,
        ) -> VerificationFuture<'a, ()> {
            Box::pin(async move {
                self.tempo_results
                    .lock()
                    .map_err(|_| CliError::Api("test verifier lock was poisoned".to_owned()))?
                    .pop_front()
                    .ok_or_else(|| CliError::Api("unexpected Tempo verification".to_owned()))?
                    .map_err(VerificationFailure::into_cli_error)
            })
        }
    }

    impl ConnectionVerifier for PendingJiraVerifier {
        fn verify_jira<'a>(
            &'a self,
            _connection: &'a JiraCredentials,
            _debug: bool,
        ) -> VerificationFuture<'a, String> {
            Box::pin(std::future::pending())
        }

        fn verify_tempo<'a>(
            &'a self,
            _connection: &'a TempoCredentials,
            _debug: bool,
        ) -> VerificationFuture<'a, ()> {
            Box::pin(async {
                Err(CliError::Api(
                    "Tempo verification should not start".to_owned(),
                ))
            })
        }
    }

    impl ConnectionVerifier for PendingTempoVerifier {
        fn verify_jira<'a>(
            &'a self,
            _connection: &'a JiraCredentials,
            _debug: bool,
        ) -> VerificationFuture<'a, String> {
            Box::pin(async { Ok("derived-account".to_owned()) })
        }

        fn verify_tempo<'a>(
            &'a self,
            _connection: &'a TempoCredentials,
            _debug: bool,
        ) -> VerificationFuture<'a, ()> {
            Box::pin(std::future::pending())
        }
    }

    impl ConnectionVerifier for DoctorVerifier {
        fn verify_jira<'a>(
            &'a self,
            _connection: &'a JiraCredentials,
            _debug: bool,
        ) -> VerificationFuture<'a, String> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .map_err(|_| CliError::Api("test verifier lock was poisoned".to_owned()))?
                    .push("jira");
                self.jira_result
                    .lock()
                    .map_err(|_| CliError::Api("test verifier lock was poisoned".to_owned()))?
                    .take()
                    .ok_or_else(|| CliError::Api("unexpected Jira verification".to_owned()))?
                    .map_err(VerificationFailure::into_cli_error)
            })
        }

        fn verify_tempo<'a>(
            &'a self,
            _connection: &'a TempoCredentials,
            _debug: bool,
        ) -> VerificationFuture<'a, ()> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .map_err(|_| CliError::Api("test verifier lock was poisoned".to_owned()))?
                    .push("tempo");
                self.tempo_result
                    .lock()
                    .map_err(|_| CliError::Api("test verifier lock was poisoned".to_owned()))?
                    .take()
                    .ok_or_else(|| CliError::Api("unexpected Tempo verification".to_owned()))?
                    .map_err(VerificationFailure::into_cli_error)
            })
        }
    }

    fn doctor_app(
        path: PathBuf,
        jira_result: Result<String, VerificationFailure>,
        tempo_result: Result<(), VerificationFailure>,
    ) -> (App, Arc<Mutex<Vec<&'static str>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let app = App::with_connection_verifier(
            path,
            DoctorVerifier {
                jira_result: Mutex::new(Some(jira_result)),
                tempo_result: Mutex::new(Some(tempo_result)),
                calls: Arc::clone(&calls),
            },
        );
        (app, calls)
    }

    fn interactive_app(
        path: PathBuf,
        state: Arc<Mutex<PromptState>>,
        jira_results: impl IntoIterator<Item = Result<String, VerificationFailure>>,
        tempo_results: impl IntoIterator<Item = Result<(), VerificationFailure>>,
    ) -> App {
        let browser_state = Arc::clone(&state);
        App::with_setup_dependencies(
            path,
            SequenceVerifier {
                jira_results: Mutex::new(jira_results.into_iter().collect()),
                tempo_results: Mutex::new(tempo_results.into_iter().collect()),
            },
            FakePrompter {
                terminal: true,
                state,
            },
            FakeBrowserLauncher {
                state: browser_state,
            },
        )
    }

    fn first_run_tui_events(save: bool) -> Vec<Event> {
        let mut events = vec![
            Event::Paste("https://Example.atlassian.net/jira/software".to_owned()),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Paste("person".to_owned()),
            Event::Key(KeyEvent::new(
                KeyCode::Char('@'),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            )),
            Event::Paste("example.com".to_owned()),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Paste("scripted-jira-secret".to_owned()),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Event::Paste("scripted-tempo-secret".to_owned()),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        ];
        if save {
            events.push(Event::Key(KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
            )));
        } else {
            events.push(Event::Key(KeyEvent::new(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
            )));
        }
        events
    }

    fn reconfiguration_tui_events() -> Vec<Event> {
        vec![
            // Retain the stored Jira credential and verify the prefilled identity.
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            // Retain the stored Tempo credential.
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            // Return from Save and replace only the Tempo credential.
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Event::Paste("replacement-tempo-token".to_owned()),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            // Return through Tempo to Jira and edit the verified identity.
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Paste(".updated".to_owned()),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Paste("replacement-jira-token".to_owned()),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            // Jira edits require Tempo to be verified again before Save.
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        ]
    }

    #[cfg(unix)]
    fn spawn_setup_pty(
        path: &std::path::Path,
        scenario: &str,
    ) -> Result<OsSession, Box<dyn std::error::Error>> {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args([
                "--exact",
                "app::tests::pty_setup_helper",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("DRAG_PTY_CONFIG", path)
            .env("DRAG_PTY_SCENARIO", scenario);
        let mut session = Session::spawn(command)?;
        session.set_expect_timeout(Some(Duration::from_secs(10)));
        Ok(session)
    }

    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "PTY child process invoked by the interactive setup tests"]
    async fn pty_setup_helper() -> Result<(), Box<dyn std::error::Error>> {
        let path = PathBuf::from(std::env::var("DRAG_PTY_CONFIG")?);
        let scenario = std::env::var("DRAG_PTY_SCENARIO")?;
        let (jira_results, tempo_results) = match scenario.as_str() {
            "success" | "reconfigure" | "late-cancel" => (
                VecDeque::from([Ok("pty-account".to_owned())]),
                VecDeque::from([Ok(())]),
            ),
            "retry" => (
                VecDeque::from([
                    Err(VerificationFailure::Authentication(
                        "Jira credentials rejected".to_owned(),
                    )),
                    Ok("pty-account".to_owned()),
                ]),
                VecDeque::from([
                    Err(VerificationFailure::Authentication(
                        "Tempo token rejected".to_owned(),
                    )),
                    Ok(()),
                ]),
            ),
            _ => return Err(format!("unknown PTY scenario: {scenario}").into()),
        };
        let app = App::with_connection_verifier(
            path,
            SequenceVerifier {
                jira_results: Mutex::new(jira_results),
                tempo_results: Mutex::new(tempo_results),
            },
        );

        match app
            .setup(SetupArgs {
                from_env: false,
                no_open: false,
            })
            .await
        {
            Ok(result) => crate::emit_result(result, ResolvedOutputMode::Json)?,
            Err(error) => crate::emit_error(&error, ResolvedOutputMode::Json),
        }
        Ok(())
    }

    impl ConnectionVerifier for FakeVerifier {
        fn verify_jira<'a>(
            &'a self,
            _connection: &'a JiraCredentials,
            _debug: bool,
        ) -> VerificationFuture<'a, String> {
            let error = self.jira_error.clone();
            Box::pin(async move {
                match error {
                    Some(message) => Err(CliError::Api(message)),
                    None => Ok("derived-account".to_owned()),
                }
            })
        }

        fn verify_tempo<'a>(
            &'a self,
            connection: &'a TempoCredentials,
            _debug: bool,
        ) -> VerificationFuture<'a, ()> {
            let account_id = connection.account_id.clone();
            let error = self.tempo_error.clone();
            let accounts = Arc::clone(&self.tempo_accounts);
            let config_update = self.config_update.clone();
            Box::pin(async move {
                accounts
                    .lock()
                    .map_err(|_| CliError::Api("test verifier lock was poisoned".to_owned()))?
                    .push(account_id);
                if let Some((path, config)) = config_update {
                    config.save(&path)?;
                }
                match error {
                    Some(message) => Err(CliError::Api(message)),
                    None => Ok(()),
                }
            })
        }
    }

    fn setup_credentials() -> SetupCredentials {
        SetupCredentials {
            tempo_token: "new-tempo-token".to_owned(),
            atlassian_user_email: "new@example.com".to_owned(),
            atlassian_token: "new-jira-token".to_owned(),
            hostname: "example.atlassian.net".to_owned(),
        }
    }

    fn existing_config() -> Config {
        Config {
            tempo_token: Some("old-tempo-token".to_owned()),
            account_id: Some("old-account".to_owned()),
            atlassian_user_email: Some("old@example.com".to_owned()),
            atlassian_token: Some("old-jira-token".to_owned()),
            hostname: Some("old.atlassian.net".to_owned()),
            aliases: BTreeMap::from([("lunch".to_owned(), "ABC-1".to_owned())]),
            trackers: BTreeMap::from([(
                "ABC-2".to_owned(),
                Tracker::new("ABC-2".to_owned(), Some("work".to_owned()), 123),
            )]),
        }
    }

    #[tokio::test]
    async fn doctor_without_remote_checks_never_calls_the_verifier(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        existing_config().save(&path)?;
        let (app, calls) = doctor_app(
            path,
            Err(VerificationFailure::Fatal(
                "Jira must not be called".to_owned(),
            )),
            Err(VerificationFailure::Fatal(
                "Tempo must not be called".to_owned(),
            )),
        );

        let result = app.doctor(DoctorArgs { remote: false }).await?;

        assert!(result.failure.is_none());
        assert!(result.data.get("remoteChecks").is_none());
        assert!(result.human.contains("Jira: configured"));
        assert!(result.human.contains("Tempo: configured"));
        assert!(calls
            .lock()
            .map_err(|_| "test verifier lock was poisoned")?
            .is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn doctor_remote_checks_report_both_connected_without_writing_config(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        existing_config().save(&path)?;
        let before = fs::read(&path)?;
        let (app, calls) = doctor_app(path.clone(), Ok("verified-account".to_owned()), Ok(()));

        let result = app.doctor(DoctorArgs { remote: true }).await?;

        assert!(result.failure.is_none());
        assert_eq!(result.data["remoteChecks"]["jira"]["status"], "connected");
        assert_eq!(result.data["remoteChecks"]["tempo"]["status"], "connected");
        assert_eq!(
            calls
                .lock()
                .map_err(|_| "test verifier lock was poisoned")?
                .as_slice(),
            ["jira", "tempo"]
        );
        assert_eq!(fs::read(path)?, before);
        Ok(())
    }

    #[tokio::test]
    async fn doctor_remote_checks_report_tempo_after_jira_failure_without_leaking_secrets(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        existing_config().save(&path)?;
        let (app, calls) = doctor_app(
            path,
            Err(VerificationFailure::Authentication(
                "old-jira-token old-tempo-token Basic-secret".to_owned(),
            )),
            Ok(()),
        );

        let result = app.doctor(DoctorArgs { remote: true }).await?;

        assert_eq!(
            result.failure.as_ref().map(|failure| failure.code),
            Some("remote_check_failed")
        );
        assert_eq!(result.exit_code(), 1);
        assert_eq!(result.data["remoteChecks"]["jira"]["status"], "failed");
        assert_eq!(
            result.data["remoteChecks"]["jira"]["errorCode"],
            "api_error"
        );
        assert_eq!(result.data["remoteChecks"]["tempo"]["status"], "connected");
        assert_eq!(
            calls
                .lock()
                .map_err(|_| "test verifier lock was poisoned")?
                .as_slice(),
            ["jira", "tempo"]
        );
        let output = format!("{} {}", result.human, result.data);
        assert!(!output.contains("old-jira-token"));
        assert!(!output.contains("old-tempo-token"));
        assert!(!output.contains("Basic-secret"));
        Ok(())
    }

    #[tokio::test]
    async fn doctor_remote_checks_report_jira_after_tempo_failure(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        existing_config().save(&path)?;
        let (app, calls) = doctor_app(
            path,
            Ok("verified-account".to_owned()),
            Err(VerificationFailure::Fatal("Tempo unavailable".to_owned())),
        );

        let result = app.doctor(DoctorArgs { remote: true }).await?;

        assert!(result.failure.is_some());
        assert_eq!(result.exit_code(), 1);
        assert_eq!(result.data["remoteChecks"]["jira"]["status"], "connected");
        assert_eq!(result.data["remoteChecks"]["tempo"]["status"], "failed");
        assert_eq!(
            calls
                .lock()
                .map_err(|_| "test verifier lock was poisoned")?
                .as_slice(),
            ["jira", "tempo"]
        );
        Ok(())
    }

    #[tokio::test]
    async fn doctor_remote_checks_report_each_missing_service_without_network_access(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let (app, calls) = doctor_app(
            path,
            Err(VerificationFailure::Fatal(
                "Jira must not be called".to_owned(),
            )),
            Err(VerificationFailure::Fatal(
                "Tempo must not be called".to_owned(),
            )),
        );

        let result = app.doctor(DoctorArgs { remote: true }).await?;

        assert!(result.failure.is_some());
        assert_eq!(result.exit_code(), 2);
        assert_eq!(
            result.data["remoteChecks"]["jira"]["status"],
            "notConfigured"
        );
        assert_eq!(
            result.data["remoteChecks"]["tempo"]["status"],
            "notConfigured"
        );
        assert!(calls
            .lock()
            .map_err(|_| "test verifier lock was poisoned")?
            .is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn doctor_remote_checks_run_a_configured_service_when_the_other_is_missing(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let mut config = existing_config();
        config.hostname = None;
        config.atlassian_user_email = None;
        config.atlassian_token = None;
        config.save(&path)?;
        let (app, calls) = doctor_app(
            path,
            Err(VerificationFailure::Fatal(
                "Jira must not be called".to_owned(),
            )),
            Ok(()),
        );

        let result = app.doctor(DoctorArgs { remote: true }).await?;

        assert_eq!(
            result.data["remoteChecks"]["jira"]["status"],
            "notConfigured"
        );
        assert_eq!(result.data["remoteChecks"]["tempo"]["status"], "connected");
        assert_eq!(
            calls
                .lock()
                .map_err(|_| "test verifier lock was poisoned")?
                .as_slice(),
            ["tempo"]
        );
        Ok(())
    }

    #[tokio::test]
    async fn doctor_remote_checks_reject_malformed_config_before_network_access(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        fs::write(&path, "{not valid json")?;
        let (app, calls) = doctor_app(
            path,
            Err(VerificationFailure::Fatal(
                "Jira must not be called".to_owned(),
            )),
            Err(VerificationFailure::Fatal(
                "Tempo must not be called".to_owned(),
            )),
        );

        let Err(error) = app.doctor(DoctorArgs { remote: true }).await else {
            return Err("malformed config should fail doctor".into());
        };

        assert!(matches!(error, CliError::Config { .. }));
        assert!(calls
            .lock()
            .map_err(|_| "test verifier lock was poisoned")?
            .is_empty());
        Ok(())
    }

    #[test]
    fn normalizes_bare_hosts_and_https_jira_urls() -> Result<(), Box<dyn std::error::Error>> {
        for (input, expected) in [
            ("EXAMPLE.atlassian.net", "example.atlassian.net"),
            (
                "https://Example.atlassian.net/jira/software/projects/ABC?view=all#top",
                "example.atlassian.net",
            ),
        ] {
            assert_eq!(normalize_jira_site(input)?, expected);
        }
        Ok(())
    }

    #[test]
    fn rejects_unsafe_jira_sites() {
        for input in [
            "",
            "http://example.atlassian.net",
            "https://user:password@example.atlassian.net",
            "https://example.atlassian.net:8443",
            "example.atlassian.net/path",
            "https://127.0.0.1",
            "bad host.atlassian.net",
        ] {
            assert!(normalize_jira_site(input).is_err(), "{input:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn pty_first_run_hides_tokens_and_emits_json_success() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let mut session = spawn_setup_pty(&path, "success")?;

        session.expect("Connect Jira")?;
        session.expect("Jira site (hostname or HTTPS URL)")?;
        session.send_line("https://Example.atlassian.net/jira/software")?;
        session.expect("Atlassian email")?;
        session.send_line("person@example.com")?;
        session.expect(ATLASSIAN_TOKEN_URL)?;
        session.expect("pasted input will not be displayed")?;
        session.send_line("pty-jira-secret")?;
        let jira_output = session.expect("Connect Tempo")?;
        assert!(!String::from_utf8_lossy(jira_output.before()).contains("pty-jira-secret"));
        session.expect("api-integration")?;
        session.expect("pasted input will not be displayed")?;
        session.send_line("pty-tempo-secret")?;
        let success_output = session.expect("\"source\": \"interactive\"")?;
        assert!(!String::from_utf8_lossy(success_output.before()).contains("pty-tempo-secret"));
        session.expect(Eof)?;

        let saved = Config::load(&path)?;
        assert_eq!(saved.hostname.as_deref(), Some("example.atlassian.net"));
        assert_eq!(saved.account_id.as_deref(), Some("pty-account"));
        assert_eq!(saved.atlassian_token.as_deref(), Some("pty-jira-secret"));
        assert_eq!(saved.tempo_token.as_deref(), Some("pty-tempo-secret"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn pty_authentication_retries_reuse_latest_jira_values_and_retry_only_tempo_token(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let mut session = spawn_setup_pty(&path, "retry")?;

        session.expect("Jira site (hostname or HTTPS URL)")?;
        session.send_line("example.atlassian.net")?;
        session.expect("Atlassian email")?;
        session.send_line("person@example.com")?;
        session.expect("pasted input will not be displayed")?;
        session.send_line("bad-jira-token")?;
        session.expect("Could not connect to Jira")?;
        session.expect("Jira site (hostname or HTTPS URL) [example.atlassian.net]")?;
        session.send_line("")?;
        session.expect("Atlassian email [person@example.com]")?;
        session.send_line("")?;
        session.expect("pasted input will not be displayed")?;
        session.send_line("good-jira-token")?;
        session.expect("Connect Tempo")?;
        session.expect("pasted input will not be displayed")?;
        session.send_line("bad-tempo-token")?;
        session.expect("Could not connect to Tempo")?;
        session.expect("pasted input will not be displayed")?;
        session.send_line("good-tempo-token")?;
        session.expect("\"source\": \"interactive\"")?;
        session.expect(Eof)?;

        let saved = Config::load(&path)?;
        assert_eq!(saved.hostname.as_deref(), Some("example.atlassian.net"));
        assert_eq!(
            saved.atlassian_user_email.as_deref(),
            Some("person@example.com")
        );
        assert_eq!(saved.atlassian_token.as_deref(), Some("good-jira-token"));
        assert_eq!(saved.tempo_token.as_deref(), Some("good-tempo-token"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn pty_reconfiguration_offers_defaults_and_retains_tokens(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        existing_config().save(&path)?;
        let mut session = spawn_setup_pty(&path, "reconfigure")?;

        session.expect("Jira site (hostname or HTTPS URL) [old.atlassian.net]")?;
        session.send_line("")?;
        session.expect("Atlassian email [old@example.com]")?;
        session.send_line("")?;
        session.expect("press Enter to keep the existing token")?;
        session.send_line("")?;
        session.expect("Connect Tempo")?;
        session.expect("press Enter to keep the existing token")?;
        session.send_line("")?;
        session.expect("\"source\": \"interactive\"")?;
        session.expect(Eof)?;

        let saved = Config::load(&path)?;
        assert_eq!(saved.atlassian_token.as_deref(), Some("old-jira-token"));
        assert_eq!(saved.tempo_token.as_deref(), Some("old-tempo-token"));
        assert!(saved.aliases.contains_key("lunch"));
        assert!(saved.trackers.contains_key("ABC-2"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn pty_late_interrupt_leaves_existing_config_unchanged(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        existing_config().save(&path)?;
        let before = fs::read(&path)?;
        let mut session = spawn_setup_pty(&path, "late-cancel")?;

        session.expect("Jira site (hostname or HTTPS URL) [old.atlassian.net]")?;
        session.send_line("")?;
        session.expect("Atlassian email [old@example.com]")?;
        session.send_line("")?;
        session.expect("press Enter to keep the existing token")?;
        session.send_line("")?;
        session.expect("Connect Tempo")?;
        session.expect("press Enter to keep the existing token")?;
        session.send(ControlCode::EndOfText)?;
        session.expect(Eof)?;

        assert_eq!(fs::read(path)?, before);
        Ok(())
    }

    #[tokio::test]
    async fn high_level_onboarding_session_drives_verification_and_transactional_save(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let initial = existing_config();
        initial.save(&path)?;
        let mut concurrent = initial;
        concurrent
            .aliases
            .insert("meeting".to_owned(), "ABC-3".to_owned());
        concurrent.trackers.insert(
            "ABC-4".to_owned(),
            Tracker::new("ABC-4".to_owned(), Some("concurrent".to_owned()), 456),
        );
        let events = Arc::new(Mutex::new(Vec::new()));
        let tempo_accounts = Arc::new(Mutex::new(Vec::new()));
        let app = App::with_onboarding_session(
            path.clone(),
            FakeVerifier {
                jira_error: None,
                tempo_error: None,
                tempo_accounts: Arc::clone(&tempo_accounts),
                config_update: Some((path.clone(), concurrent)),
            },
            ScriptedOnboardingSession {
                events: Arc::clone(&events),
            },
        );

        app.setup(SetupArgs {
            from_env: false,
            no_open: true,
        })
        .await?;

        let saved = Config::load(&path)?;
        let observed = (
            saved.hostname.as_deref(),
            saved.atlassian_user_email.as_deref(),
            saved.atlassian_token.as_deref(),
            saved.tempo_token.as_deref(),
            saved.account_id.as_deref(),
            saved.aliases.get("meeting").map(String::as_str),
        );
        assert_eq!(
            observed,
            (
                Some("example.atlassian.net"),
                Some("scripted@example.com"),
                Some("scripted-jira-token"),
                Some("scripted-tempo-token"),
                Some("derived-account"),
                Some("ABC-3"),
            )
        );
        assert_eq!(
            tempo_accounts
                .lock()
                .map_err(|_| "test verifier lock was poisoned")?
                .as_slice(),
            ["derived-account"]
        );
        assert_eq!(
            events
                .lock()
                .map_err(|_| "test session lock was poisoned")?
                .as_slice(),
            ["jira-browser:false", "tempo-browser:false", "save"]
        );
        assert!(saved.trackers.contains_key("ABC-2"));
        assert!(saved.trackers.contains_key("ABC-4"));
        Ok(())
    }

    #[tokio::test]
    async fn ratatui_first_run_masks_secrets_verifies_and_saves_from_scripted_events(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let prompt_state = Arc::new(Mutex::new(PromptState {
            browser_failure: Some("no default browser".to_owned()),
            ..PromptState::default()
        }));
        let frames = Arc::new(Mutex::new(Vec::new()));
        let app = App::with_onboarding_session(
            path.clone(),
            SequenceVerifier {
                jira_results: Mutex::new(VecDeque::from([Ok("derived-account".to_owned())])),
                tempo_results: Mutex::new(VecDeque::from([Ok(())])),
            },
            RatatuiOnboardingSession::scripted(
                FakeBrowserLauncher {
                    state: Arc::clone(&prompt_state),
                },
                first_run_tui_events(true),
                Arc::clone(&frames),
            ),
        );

        let result = app
            .setup(SetupArgs {
                from_env: false,
                no_open: false,
            })
            .await?;

        let saved = Config::load(&path)?;
        assert_eq!(
            (
                saved.hostname.as_deref(),
                saved.atlassian_user_email.as_deref(),
                saved.atlassian_token.as_deref(),
                saved.tempo_token.as_deref(),
                saved.account_id.as_deref(),
            ),
            (
                Some("example.atlassian.net"),
                Some("person@example.com"),
                Some("scripted-jira-secret"),
                Some("scripted-tempo-secret"),
                Some("derived-account"),
            )
        );
        assert_eq!(result.data["source"], "interactive");

        let captured_frames = frames.lock().map_err(|_| "test frame lock poisoned")?;
        assert!(captured_frames
            .iter()
            .any(|frame| frame.contains("Warning: Could not open")));
        assert!(!captured_frames
            .last()
            .ok_or("Ratatui did not render a Save frame")?
            .contains("Warning:"));
        let frames = captured_frames.join("\n--- frame ---\n");
        for visible in [
            "Connect Jira",
            "Connect Tempo",
            "Save",
            "Verifying Connect Jira",
            "Verifying Connect Tempo",
            "example.atlassian.net",
            "person@example.com",
            ATLASSIAN_TOKEN_URL,
            "api-integration",
            "Nothing has been saved yet",
        ] {
            assert!(frames.contains(visible), "missing rendered text: {visible}");
        }
        for secret in [
            "scripted-jira-secret",
            "scripted-tempo-secret",
            "derived-account",
        ] {
            assert!(!frames.contains(secret), "rendered secret: {secret}");
        }

        let prompt_state = prompt_state
            .lock()
            .map_err(|_| "test browser lock poisoned")?;
        assert_eq!(
            prompt_state.browser_urls,
            [
                ATLASSIAN_TOKEN_URL,
                "https://example.atlassian.net/plugins/servlet/ac/io.tempo.jira/tempo-app#!/configuration/api-integration",
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn ratatui_first_run_does_not_write_before_explicit_save(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let app = App::with_onboarding_session(
            path.clone(),
            SequenceVerifier {
                jira_results: Mutex::new(VecDeque::from([Ok("derived-account".to_owned())])),
                tempo_results: Mutex::new(VecDeque::from([Ok(())])),
            },
            RatatuiOnboardingSession::scripted(
                NoopBrowserLauncher,
                first_run_tui_events(false),
                Arc::new(Mutex::new(Vec::new())),
            ),
        );

        let error = app
            .setup(SetupArgs {
                from_env: false,
                no_open: true,
            })
            .await
            .err()
            .ok_or("setup unexpectedly saved without the Save action")?;

        assert!(error.to_string().contains("cancelled"));
        assert!(!path.exists());
        Ok(())
    }

    #[tokio::test]
    async fn ratatui_reconfiguration_retains_replaces_backtracks_and_reverifies(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        existing_config().save(&path)?;
        let frames = Arc::new(Mutex::new(Vec::new()));
        let app = App::with_onboarding_session(
            path.clone(),
            SequenceVerifier {
                jira_results: Mutex::new(VecDeque::from([
                    Ok("initial-derived-account".to_owned()),
                    Ok("final-derived-account".to_owned()),
                ])),
                tempo_results: Mutex::new(VecDeque::from([Ok(()), Ok(()), Ok(())])),
            },
            RatatuiOnboardingSession::scripted(
                NoopBrowserLauncher,
                reconfiguration_tui_events(),
                Arc::clone(&frames),
            ),
        );

        let result = app
            .setup(SetupArgs {
                from_env: false,
                no_open: true,
            })
            .await?;

        let saved = Config::load(&path)?;
        assert_eq!(
            (
                saved.hostname.as_deref(),
                saved.atlassian_user_email.as_deref(),
                saved.atlassian_token.as_deref(),
                saved.tempo_token.as_deref(),
                saved.account_id.as_deref(),
            ),
            (
                Some("old.atlassian.net"),
                Some("old@example.com.updated"),
                Some("replacement-jira-token"),
                Some("replacement-tempo-token"),
                Some("final-derived-account"),
            )
        );
        assert!(saved.aliases.contains_key("lunch"));
        assert!(saved.trackers.contains_key("ABC-2"));

        let captured_frames = frames.lock().map_err(|_| "test frame lock poisoned")?;
        assert!(captured_frames.first().is_some_and(|frame| {
            frame.contains("old.atlassian.net")
                && frame.contains("old@example.com")
                && frame.contains("Stored credential available")
                && frame.contains("Esc")
                && frame.contains("cancel")
        }));
        assert!(captured_frames.iter().any(|frame| {
            frame.contains("Connect Tempo")
                && frame.contains("Stored credential available")
                && frame.contains("Esc")
                && frame.contains("back")
        }));
        assert!(captured_frames.iter().any(|frame| {
            frame.contains("✓ Connect Jira") && frame.contains("› Connect Tempo")
        }));
        assert!(captured_frames.iter().any(|frame| {
            frame.contains("old@example.com.updated")
                && frame.contains("› Connect Jira")
                && frame.contains("○ Connect Tempo")
        }));
        assert!(captured_frames.last().is_some_and(|frame| {
            frame.contains("old@example.com.updated")
                && frame.contains("✓ Jira connected")
                && frame.contains("✓ Tempo connected")
        }));

        let rendered = format!("{} {}", result.human, result.data);
        for secret in [
            "old-jira-token",
            "old-tempo-token",
            "replacement-jira-token",
            "replacement-tempo-token",
            "old-account",
            "initial-derived-account",
            "final-derived-account",
        ] {
            assert!(!captured_frames.iter().any(|frame| frame.contains(secret)));
            assert!(!rendered.contains(secret));
        }
        Ok(())
    }

    #[tokio::test]
    async fn ratatui_backtracking_without_edits_does_not_repeat_verification(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        existing_config().save(&path)?;
        let frames = Arc::new(Mutex::new(Vec::new()));
        let events = vec![
            // Complete setup once with retained credentials.
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            // Navigate back to Jira without editing anything.
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            // Continue through the still-connected stages and save.
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        ];
        let app = App::with_onboarding_session(
            path.clone(),
            SequenceVerifier {
                jira_results: Mutex::new(VecDeque::from([Ok("derived-account".to_owned())])),
                tempo_results: Mutex::new(VecDeque::from([Ok(())])),
            },
            RatatuiOnboardingSession::scripted(NoopBrowserLauncher, events, Arc::clone(&frames)),
        );

        app.setup(SetupArgs {
            from_env: false,
            no_open: true,
        })
        .await?;

        let saved = Config::load(&path)?;
        assert_eq!(saved.account_id.as_deref(), Some("derived-account"));
        assert!(frames
            .lock()
            .map_err(|_| "test frame lock poisoned")?
            .iter()
            .any(|frame| {
                frame.contains("✓ Connect Jira")
                    && frame.contains("✓ Connect Tempo")
                    && frame.contains("continue")
            }));
        Ok(())
    }

    #[tokio::test]
    async fn ratatui_backtracking_discards_an_unverified_tempo_token_buffer(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        existing_config().save(&path)?;
        let frames = Arc::new(Mutex::new(Vec::new()));
        let events = vec![
            // Reach Save with both stored credentials verified.
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            // Start a replacement, then leave Tempo without verifying it.
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Event::Paste("partial-tempo-token".to_owned()),
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            // Continue through Jira and retain the stored Tempo credential.
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        ];
        let app = App::with_onboarding_session(
            path.clone(),
            SequenceVerifier {
                jira_results: Mutex::new(VecDeque::from([Ok("derived-account".to_owned())])),
                tempo_results: Mutex::new(VecDeque::from([Ok(()), Ok(())])),
            },
            RatatuiOnboardingSession::scripted(NoopBrowserLauncher, events, Arc::clone(&frames)),
        );

        app.setup(SetupArgs {
            from_env: false,
            no_open: true,
        })
        .await?;

        let saved = Config::load(&path)?;
        assert_eq!(saved.tempo_token.as_deref(), Some("old-tempo-token"));
        assert!(frames
            .lock()
            .map_err(|_| "test frame lock poisoned")?
            .iter()
            .any(|frame| {
                frame.contains("Connect Tempo") && frame.contains("Stored credential available")
            }));
        Ok(())
    }

    #[tokio::test]
    async fn ratatui_pending_tempo_back_discards_the_unverified_token_buffer(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        existing_config().save(&path)?;
        let before = fs::read(&path)?;
        let frames = Arc::new(Mutex::new(Vec::new()));
        let events = vec![
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Event::Paste("partial-tempo-token".to_owned()),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            // Continue through the still-connected Jira stage, then cancel on Tempo.
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        ];
        let app = App::with_onboarding_session(
            path.clone(),
            PendingTempoVerifier,
            RatatuiOnboardingSession::scripted(NoopBrowserLauncher, events, Arc::clone(&frames)),
        );

        let error = app
            .setup(SetupArgs {
                from_env: false,
                no_open: true,
            })
            .await
            .err()
            .ok_or("pending Tempo setup unexpectedly succeeded")?;

        assert!(error.to_string().contains("cancelled"));
        assert_eq!(fs::read(path)?, before);
        assert!(frames
            .lock()
            .map_err(|_| "test frame lock poisoned")?
            .iter()
            .any(|frame| {
                frame.contains("Connect Tempo") && frame.contains("Stored credential available")
            }));
        Ok(())
    }

    #[tokio::test]
    async fn ratatui_reconfiguration_cancellation_leaves_config_byte_for_byte_unchanged(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        existing_config().save(&path)?;
        let before = fs::read(&path)?;
        let events = vec![
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        ];
        let app = App::with_onboarding_session(
            path.clone(),
            SequenceVerifier {
                jira_results: Mutex::new(VecDeque::from([Ok("derived-account".to_owned())])),
                tempo_results: Mutex::new(VecDeque::from([Ok(())])),
            },
            RatatuiOnboardingSession::scripted(
                NoopBrowserLauncher,
                events,
                Arc::new(Mutex::new(Vec::new())),
            ),
        );

        let error = app
            .setup(SetupArgs {
                from_env: false,
                no_open: true,
            })
            .await
            .err()
            .ok_or("reconfiguration unexpectedly saved after cancellation")?;

        assert!(error.to_string().contains("cancelled"));
        assert_eq!(fs::read(path)?, before);
        Ok(())
    }

    #[tokio::test]
    async fn ratatui_verification_keeps_terminal_events_responsive(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let frames = Arc::new(Mutex::new(Vec::new()));
        let mut events = first_run_tui_events(true);
        events.truncate(11);
        events.push(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        let app = App::with_onboarding_session(
            path.clone(),
            PendingJiraVerifier,
            RatatuiOnboardingSession::scripted(NoopBrowserLauncher, events, Arc::clone(&frames)),
        );

        let error = app
            .setup(SetupArgs {
                from_env: false,
                no_open: true,
            })
            .await
            .err()
            .ok_or("pending Jira verification ignored cancellation")?;

        assert!(error.to_string().contains("cancelled"));
        assert!(!path.exists());
        assert!(frames
            .lock()
            .map_err(|_| "test frame lock poisoned")?
            .iter()
            .any(|frame| frame.contains("Verifying Connect Jira")));
        Ok(())
    }

    #[tokio::test]
    async fn incomplete_onboarding_session_cannot_save_credentials(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let initial = existing_config();
        initial.save(&path)?;
        let before = fs::read(&path)?;
        let app = App::with_onboarding_session(
            path.clone(),
            FakeVerifier {
                jira_error: None,
                tempo_error: None,
                tempo_accounts: Arc::new(Mutex::new(Vec::new())),
                config_update: None,
            },
            IncompleteOnboardingSession,
        );

        let error = app
            .setup(SetupArgs {
                from_env: false,
                no_open: true,
            })
            .await
            .err()
            .ok_or("incomplete onboarding unexpectedly succeeded")?;

        assert_eq!(
            (error.to_string(), fs::read(path)?),
            ("invalid onboarding workflow state".to_owned(), before)
        );
        Ok(())
    }

    #[tokio::test]
    async fn interactive_setup_connects_both_services_and_saves_once_complete(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let state = Arc::new(Mutex::new(PromptState {
            text_responses: VecDeque::from([
                "https://Example.atlassian.net/jira/software/projects/DRAG".to_owned(),
                "person@example.com".to_owned(),
            ]),
            secret_responses: VecDeque::from([
                Some("jira-secret".to_owned()),
                Some("tempo-secret".to_owned()),
            ]),
            ..PromptState::default()
        }));
        let app = interactive_app(
            path.clone(),
            Arc::clone(&state),
            [Ok("derived-account".to_owned())],
            [Ok(())],
        );

        let result = app
            .setup(SetupArgs {
                from_env: false,
                no_open: false,
            })
            .await?;

        let saved = Config::load(&path)?;
        assert_eq!(saved.hostname.as_deref(), Some("example.atlassian.net"));
        assert_eq!(saved.account_id.as_deref(), Some("derived-account"));
        assert_eq!(saved.atlassian_token.as_deref(), Some("jira-secret"));
        assert_eq!(saved.tempo_token.as_deref(), Some("tempo-secret"));
        assert_eq!(result.data["source"], "interactive");
        assert_eq!(result.data["connection"]["jira"]["status"], "connected");
        assert_eq!(result.data["connection"]["tempo"]["status"], "connected");
        let output = format!("{} {}", result.human, result.data);
        assert!(!output.contains("derived-account"));
        assert!(!output.contains("jira-secret"));
        assert!(!output.contains("tempo-secret"));
        assert!(!output.contains(ATLASSIAN_TOKEN_URL));
        let state = state.lock().map_err(|_| "test prompt lock was poisoned")?;
        assert_eq!(
            state
                .text_prompts
                .iter()
                .map(|(label, _)| label.as_str())
                .collect::<Vec<_>>(),
            ["Jira site (hostname or HTTPS URL)", "Atlassian email"]
        );
        assert!(state
            .messages
            .iter()
            .any(|message| message.contains(ATLASSIAN_TOKEN_URL)));
        assert!(state.messages.iter().any(|message| message.contains(
            "https://example.atlassian.net/plugins/servlet/ac/io.tempo.jira/tempo-app#!/configuration/api-integration"
        )));
        assert_eq!(
            state.browser_urls,
            [
                ATLASSIAN_TOKEN_URL,
                "https://example.atlassian.net/plugins/servlet/ac/io.tempo.jira/tempo-app#!/configuration/api-integration"
            ]
        );
        assert_eq!(
            state
                .events
                .iter()
                .filter(|event| {
                    event.starts_with("message:Create or manage")
                        || event.starts_with("browser:")
                        || event.starts_with("secret:")
                })
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [
                "message:Create or manage your Atlassian API token:\nhttps://id.atlassian.com/manage-profile/security/api-tokens",
                "browser:https://id.atlassian.com/manage-profile/security/api-tokens",
                "secret:Atlassian API token",
                "message:Create or manage your Tempo API token:\nhttps://example.atlassian.net/plugins/servlet/ac/io.tempo.jira/tempo-app#!/configuration/api-integration",
                "browser:https://example.atlassian.net/plugins/servlet/ac/io.tempo.jira/tempo-app#!/configuration/api-integration",
                "secret:Tempo API token"
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn interactive_setup_no_open_prints_links_without_launching_browser(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let state = Arc::new(Mutex::new(PromptState {
            text_responses: VecDeque::from([
                "example.atlassian.net".to_owned(),
                "person@example.com".to_owned(),
            ]),
            secret_responses: VecDeque::from([
                Some("jira-secret".to_owned()),
                Some("tempo-secret".to_owned()),
            ]),
            ..PromptState::default()
        }));
        let app = interactive_app(
            path,
            Arc::clone(&state),
            [Ok("derived-account".to_owned())],
            [Ok(())],
        );

        app.setup(SetupArgs {
            from_env: false,
            no_open: true,
        })
        .await?;

        let state = state.lock().map_err(|_| "test prompt lock was poisoned")?;
        assert!(state.browser_urls.is_empty());
        assert!(state
            .messages
            .iter()
            .any(|message| message.contains(ATLASSIAN_TOKEN_URL)));
        assert!(state.messages.iter().any(|message| message.contains(
            "https://example.atlassian.net/plugins/servlet/ac/io.tempo.jira/tempo-app#!/configuration/api-integration"
        )));
        Ok(())
    }

    #[tokio::test]
    async fn browser_launch_failure_warns_and_allows_setup_to_finish(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let state = Arc::new(Mutex::new(PromptState {
            text_responses: VecDeque::from([
                "example.atlassian.net".to_owned(),
                "person@example.com".to_owned(),
            ]),
            secret_responses: VecDeque::from([
                Some("jira-secret".to_owned()),
                Some("tempo-secret".to_owned()),
            ]),
            browser_failure: Some("no default browser".to_owned()),
            ..PromptState::default()
        }));
        let app = interactive_app(
            path.clone(),
            Arc::clone(&state),
            [Ok("derived-account".to_owned())],
            [Ok(())],
        );

        let result = app
            .setup(SetupArgs {
                from_env: false,
                no_open: false,
            })
            .await?;

        assert!(path.exists());
        let output = format!("{} {}", result.human, result.data);
        assert!(!output.contains("no default browser"));
        assert!(!output.contains(ATLASSIAN_TOKEN_URL));
        let state = state.lock().map_err(|_| "test prompt lock was poisoned")?;
        assert_eq!(state.browser_urls.len(), 2);
        assert_eq!(
            state
                .messages
                .iter()
                .filter(|message| message.starts_with("Warning: could not open"))
                .count(),
            2
        );
        Ok(())
    }

    #[tokio::test]
    async fn environment_setup_never_launches_or_prompts_with_any_no_open_value(
    ) -> Result<(), Box<dyn std::error::Error>> {
        for no_open in [false, true] {
            let directory = TempDir::new()?;
            let path = directory.path().join("config.json");
            let state = Arc::new(Mutex::new(PromptState::default()));
            let mut app = interactive_app(
                path,
                Arc::clone(&state),
                [Ok("derived-account".to_owned())],
                [Ok(())],
            );
            app.connection_environment = Box::new(FakeConnectionEnvironment {
                values: BTreeMap::from([
                    (
                        "ATLASSIAN_HOST".to_owned(),
                        "example.atlassian.net".to_owned(),
                    ),
                    (
                        "ATLASSIAN_EMAIL".to_owned(),
                        "person@example.com".to_owned(),
                    ),
                    ("ATLASSIAN_TOKEN".to_owned(), "jira-secret".to_owned()),
                    ("TEMPO_TOKEN".to_owned(), "tempo-secret".to_owned()),
                ]),
            });

            app.setup(SetupArgs {
                from_env: true,
                no_open,
            })
            .await?;

            let state = state.lock().map_err(|_| "test prompt lock was poisoned")?;
            assert!(state.browser_urls.is_empty());
            assert!(state.text_prompts.is_empty());
            assert!(state.secret_prompts.is_empty());
            assert!(state.messages.is_empty());
        }
        Ok(())
    }

    #[tokio::test]
    async fn interactive_reconfiguration_offers_defaults_and_retains_hidden_tokens(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        existing_config().save(&path)?;
        let state = Arc::new(Mutex::new(PromptState {
            text_responses: VecDeque::from([
                "old.atlassian.net".to_owned(),
                "old@example.com".to_owned(),
            ]),
            secret_responses: VecDeque::from([None, None]),
            ..PromptState::default()
        }));
        let app = interactive_app(
            path.clone(),
            Arc::clone(&state),
            [Ok("new-derived-account".to_owned())],
            [Ok(())],
        );

        app.setup(SetupArgs {
            from_env: false,
            no_open: false,
        })
        .await?;

        let saved = Config::load(&path)?;
        assert_eq!(saved.atlassian_token.as_deref(), Some("old-jira-token"));
        assert_eq!(saved.tempo_token.as_deref(), Some("old-tempo-token"));
        assert!(saved.aliases.contains_key("lunch"));
        assert!(saved.trackers.contains_key("ABC-2"));
        let state = state.lock().map_err(|_| "test prompt lock was poisoned")?;
        assert_eq!(
            state.text_prompts,
            [
                (
                    "Jira site (hostname or HTTPS URL)".to_owned(),
                    Some("old.atlassian.net".to_owned())
                ),
                (
                    "Atlassian email".to_owned(),
                    Some("old@example.com".to_owned())
                )
            ]
        );
        assert_eq!(
            state.secret_prompts,
            [
                ("Atlassian API token".to_owned(), true),
                ("Tempo API token".to_owned(), true)
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn interactive_setup_retries_only_the_failed_connection(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let state = Arc::new(Mutex::new(PromptState {
            text_responses: VecDeque::from([
                "not a host".to_owned(),
                "example.atlassian.net".to_owned(),
                "person@example.com".to_owned(),
                String::new(),
                String::new(),
            ]),
            secret_responses: VecDeque::from([
                Some("bad-jira".to_owned()),
                Some("good-jira".to_owned()),
                Some("bad-tempo".to_owned()),
                Some("good-tempo".to_owned()),
            ]),
            ..PromptState::default()
        }));
        let app = interactive_app(
            path.clone(),
            Arc::clone(&state),
            [
                Err(VerificationFailure::Authentication(
                    "authentication failed".to_owned(),
                )),
                Ok("derived-account".to_owned()),
            ],
            [
                Err(VerificationFailure::Authentication(
                    "token rejected".to_owned(),
                )),
                Ok(()),
            ],
        );

        app.setup(SetupArgs {
            from_env: false,
            no_open: false,
        })
        .await?;

        let saved = Config::load(&path)?;
        assert_eq!(saved.atlassian_token.as_deref(), Some("good-jira"));
        assert_eq!(saved.tempo_token.as_deref(), Some("good-tempo"));
        let state = state.lock().map_err(|_| "test prompt lock was poisoned")?;
        let messages = &state.messages;
        assert!(messages
            .iter()
            .any(|message| message.contains("Invalid Jira site")));
        assert!(messages
            .iter()
            .any(|message| message.contains("Could not connect to Jira")));
        assert!(messages
            .iter()
            .any(|message| message.contains("Could not connect to Tempo")));
        assert_eq!(
            state.text_prompts[3..],
            [
                (
                    "Jira site (hostname or HTTPS URL)".to_owned(),
                    Some("example.atlassian.net".to_owned())
                ),
                (
                    "Atlassian email".to_owned(),
                    Some("person@example.com".to_owned())
                )
            ]
        );
        assert_eq!(
            state.browser_urls,
            [
                ATLASSIAN_TOKEN_URL,
                "https://example.atlassian.net/plugins/servlet/ac/io.tempo.jira/tempo-app#!/configuration/api-integration"
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn interactive_setup_propagates_non_authentication_verification_errors(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let state = Arc::new(Mutex::new(PromptState {
            text_responses: VecDeque::from([
                "example.atlassian.net".to_owned(),
                "person@example.com".to_owned(),
            ]),
            secret_responses: VecDeque::from([Some("jira-token".to_owned())]),
            ..PromptState::default()
        }));
        let app = interactive_app(
            path.clone(),
            Arc::clone(&state),
            [Err(VerificationFailure::Fatal(
                "network timeout".to_owned(),
            ))],
            std::iter::empty(),
        );

        let error = match app
            .setup(SetupArgs {
                from_env: false,
                no_open: false,
            })
            .await
        {
            Ok(_) => return Err("setup should propagate the network error".into()),
            Err(error) => error,
        };

        assert!(matches!(error, CliError::Api(message) if message == "network timeout"));
        assert!(!path.exists());
        let state = state.lock().map_err(|_| "test prompt lock was poisoned")?;
        assert_eq!(state.text_prompts.len(), 2);
        assert!(!state
            .messages
            .iter()
            .any(|message| message.contains("try again")));
        Ok(())
    }

    #[tokio::test]
    async fn interactive_setup_does_not_retry_fatal_tempo_errors(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let state = Arc::new(Mutex::new(PromptState {
            text_responses: VecDeque::from([
                "example.atlassian.net".to_owned(),
                "person@example.com".to_owned(),
            ]),
            secret_responses: VecDeque::from([
                Some("jira-token".to_owned()),
                Some("tempo-token".to_owned()),
            ]),
            ..PromptState::default()
        }));
        let app = interactive_app(
            path.clone(),
            Arc::clone(&state),
            [Ok("derived-account".to_owned())],
            [Err(VerificationFailure::Fatal(
                "malformed response".to_owned(),
            ))],
        );

        let error = match app
            .setup(SetupArgs {
                from_env: false,
                no_open: false,
            })
            .await
        {
            Ok(_) => return Err("setup should propagate the response error".into()),
            Err(error) => error,
        };

        assert!(matches!(error, CliError::Api(message) if message == "malformed response"));
        assert!(!path.exists());
        let state = state.lock().map_err(|_| "test prompt lock was poisoned")?;
        assert_eq!(state.secret_prompts.len(), 2);
        assert!(!state
            .messages
            .iter()
            .any(|message| message.contains("Check the Tempo token")));
        Ok(())
    }

    #[tokio::test]
    async fn interactive_cancellation_leaves_existing_config_unchanged(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        existing_config().save(&path)?;
        let before = fs::read(&path)?;
        let state = Arc::new(Mutex::new(PromptState::default()));
        let app = interactive_app(path.clone(), state, std::iter::empty(), std::iter::empty());

        let error = match app
            .setup(SetupArgs {
                from_env: false,
                no_open: false,
            })
            .await
        {
            Ok(_) => return Err("setup should be cancelled when input ends".into()),
            Err(error) => error,
        };

        assert!(error.to_string().contains("cancelled"));
        assert_eq!(fs::read(path)?, before);
        Ok(())
    }

    #[tokio::test]
    async fn cancellation_after_a_failed_connection_check_leaves_config_unchanged(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        existing_config().save(&path)?;
        let before = fs::read(&path)?;
        let state = Arc::new(Mutex::new(PromptState {
            text_responses: VecDeque::from([
                "old.atlassian.net".to_owned(),
                "old@example.com".to_owned(),
            ]),
            secret_responses: VecDeque::from([None]),
            ..PromptState::default()
        }));
        let app = interactive_app(
            path.clone(),
            state,
            [Err(VerificationFailure::Authentication(
                "authentication failed".to_owned(),
            ))],
            std::iter::empty(),
        );

        assert!(app
            .setup(SetupArgs {
                from_env: false,
                no_open: false,
            })
            .await
            .is_err());

        assert_eq!(fs::read(path)?, before);
        Ok(())
    }

    #[test]
    fn setup_environment_does_not_read_the_compatibility_account_id(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let values = BTreeMap::from([
            ("ATLASSIAN_HOST", "example.atlassian.net"),
            ("ATLASSIAN_EMAIL", "person@example.com"),
            ("ATLASSIAN_TOKEN", " jira-secret\n"),
            ("TEMPO_TOKEN", " tempo-secret\n"),
            ("TEMPO_ACCOUNT_ID", "must-not-be-used"),
        ]);
        let mut requested = Vec::new();
        let credentials = SetupCredentials::from_source(|name| {
            requested.push(name.to_owned());
            values
                .get(name)
                .map(|value| (*value).to_owned())
                .ok_or_else(|| CliError::InvalidInput(format!("missing {name}")))
        })?;

        assert_eq!(credentials.hostname, "example.atlassian.net");
        assert_eq!(credentials.atlassian_token, "jira-secret");
        assert_eq!(credentials.tempo_token, "tempo-secret");
        assert_eq!(
            requested,
            [
                "ATLASSIAN_HOST",
                "ATLASSIAN_EMAIL",
                "ATLASSIAN_TOKEN",
                "TEMPO_TOKEN"
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn verified_environment_setup_derives_account_and_preserves_local_state(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        let config = existing_config();
        config.save(&path)?;
        let tempo_accounts = Arc::new(Mutex::new(Vec::new()));
        let app = App::with_connection_verifier(
            path.clone(),
            FakeVerifier {
                jira_error: None,
                tempo_error: None,
                tempo_accounts: Arc::clone(&tempo_accounts),
                config_update: None,
            },
        );

        let result = app
            .verify_and_save_environment_setup(setup_credentials())
            .await?;

        let saved = Config::load(&path)?;
        assert_eq!(saved.account_id.as_deref(), Some("derived-account"));
        assert_eq!(saved.tempo_token.as_deref(), Some("new-tempo-token"));
        assert_eq!(
            saved.aliases.get("lunch").map(String::as_str),
            Some("ABC-1")
        );
        assert!(saved.trackers.contains_key("ABC-2"));
        let accounts = tempo_accounts
            .lock()
            .map_err(|_| "test verifier lock was poisoned")?;
        assert_eq!(accounts.as_slice(), ["derived-account"]);
        assert_eq!(result.data["source"], "environment");
        assert_eq!(result.data["verification"]["jira"], "connected");
        assert_eq!(result.data["verification"]["tempo"], "connected");
        let output = format!("{} {}", result.human, result.data);
        assert!(!output.contains("new-tempo-token"));
        assert!(!output.contains("new-jira-token"));
        assert!(!output.contains("derived-account"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
        }
        Ok(())
    }

    #[tokio::test]
    async fn verified_environment_setup_preserves_config_updates_made_during_verification(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = TempDir::new()?;
        let path = directory.path().join("config.json");
        existing_config().save(&path)?;
        let mut updated_config = existing_config();
        updated_config
            .aliases
            .insert("meeting".to_owned(), "ABC-3".to_owned());
        let app = App::with_connection_verifier(
            path.clone(),
            FakeVerifier {
                jira_error: None,
                tempo_error: None,
                tempo_accounts: Arc::new(Mutex::new(Vec::new())),
                config_update: Some((path.clone(), updated_config)),
            },
        );

        app.verify_and_save_environment_setup(setup_credentials())
            .await?;

        let saved = Config::load(&path)?;
        assert_eq!(
            saved.aliases.get("meeting").map(String::as_str),
            Some("ABC-3")
        );
        Ok(())
    }

    #[tokio::test]
    async fn failed_verification_leaves_config_byte_for_byte_unchanged(
    ) -> Result<(), Box<dyn std::error::Error>> {
        for (jira_error, tempo_error) in [
            (Some("jira rejected credentials".to_owned()), None),
            (None, Some("tempo rejected credentials".to_owned())),
        ] {
            let directory = TempDir::new()?;
            let path = directory.path().join("config.json");
            let config = existing_config();
            config.save(&path)?;
            let before = fs::read(&path)?;
            let tempo_accounts = Arc::new(Mutex::new(Vec::new()));
            let jira_should_fail = jira_error.is_some();
            let app = App::with_connection_verifier(
                path.clone(),
                FakeVerifier {
                    jira_error,
                    tempo_error,
                    tempo_accounts: Arc::clone(&tempo_accounts),
                    config_update: None,
                },
            );

            assert!(app
                .verify_and_save_environment_setup(setup_credentials())
                .await
                .is_err());
            assert_eq!(fs::read(path)?, before);
            let accounts = tempo_accounts
                .lock()
                .map_err(|_| "test verifier lock was poisoned")?;
            if jira_should_fail {
                assert!(accounts.is_empty());
            } else {
                assert_eq!(accounts.as_slice(), ["derived-account"]);
            }
        }
        Ok(())
    }
}
