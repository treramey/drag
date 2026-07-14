use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::future::Future;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::pin::Pin;

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
    AliasDeleteArgs, AliasSetArgs, DeleteArgs, ListArgs, LogArgs, LogInput, SetupArgs,
    TrackerIssueArgs, TrackerStartArgs, TrackerStopArgs,
};
use crate::config::{normalize_jira_site, Config, Credentials};
use crate::{CliError, Rendered};

pub struct App {
    path: PathBuf,
    timezone: Tz,
    debug: bool,
    connection_verifier: Box<dyn ConnectionVerifier>,
}

type VerificationFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, CliError>> + Send + 'a>>;

trait ConnectionVerifier: Send + Sync {
    fn verify_jira<'a>(
        &'a self,
        credentials: &'a SetupCredentials,
        debug: bool,
    ) -> VerificationFuture<'a, String>;

    fn verify_tempo<'a>(
        &'a self,
        credentials: &'a Credentials,
        debug: bool,
    ) -> VerificationFuture<'a, ()>;
}

struct RemoteConnectionVerifier;

impl ConnectionVerifier for RemoteConnectionVerifier {
    fn verify_jira<'a>(
        &'a self,
        credentials: &'a SetupCredentials,
        debug: bool,
    ) -> VerificationFuture<'a, String> {
        Box::pin(async move {
            let api = ApiClient::new(credentials.to_credentials(String::new()), debug)?;
            api.get_current_user_account_id().await
        })
    }

    fn verify_tempo<'a>(
        &'a self,
        credentials: &'a Credentials,
        debug: bool,
    ) -> VerificationFuture<'a, ()> {
        Box::pin(async move {
            let api = ApiClient::new(credentials.clone(), debug)?;
            api.verify_tempo_connection().await
        })
    }
}

struct SetupCredentials {
    tempo_token: String,
    atlassian_user_email: String,
    atlassian_token: String,
    hostname: String,
}

impl SetupCredentials {
    fn from_environment() -> Result<Self, CliError> {
        Self::from_source(required_setup_environment)
    }

    fn from_source(
        mut source: impl FnMut(&str) -> Result<String, CliError>,
    ) -> Result<Self, CliError> {
        let hostname = normalize_jira_site(&source("ATLASSIAN_HOST")?)?;
        let atlassian_user_email = source("ATLASSIAN_EMAIL")?.trim().to_owned();
        let atlassian_token = source("ATLASSIAN_TOKEN")?.trim().to_owned();
        let tempo_token = source("TEMPO_TOKEN")?.trim().to_owned();
        Ok(Self {
            tempo_token,
            atlassian_user_email,
            atlassian_token,
            hostname,
        })
    }

    fn to_credentials(&self, account_id: String) -> Credentials {
        Credentials {
            tempo_token: self.tempo_token.clone(),
            account_id,
            atlassian_user_email: self.atlassian_user_email.clone(),
            atlassian_token: self.atlassian_token.clone(),
            hostname: self.hostname.clone(),
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
        }
    }

    pub async fn setup(&self, args: SetupArgs) -> Result<Rendered, CliError> {
        if args.from_env {
            Config::load(&self.path)?;
            let setup_credentials = SetupCredentials::from_environment()?;
            return self
                .verify_and_save_environment_setup(setup_credentials)
                .await;
        }

        let mut config = Config::load(&self.path)?;
        let credentials = prompt_credentials()?;
        config.tempo_token = Some(credentials.tempo_token);
        config.account_id = Some(credentials.account_id);
        config.atlassian_user_email = Some(credentials.atlassian_user_email);
        config.atlassian_token = Some(credentials.atlassian_token);
        config.hostname = Some(credentials.hostname);
        config.save(&self.path)?;
        Ok(Rendered::new(
            json!({"configured": true, "path": self.path}),
            format!(
                "Setup completed successfully. Configuration saved to {}.",
                self.path.display()
            ),
        ))
    }

    async fn verify_and_save_environment_setup(
        &self,
        setup_credentials: SetupCredentials,
    ) -> Result<Rendered, CliError> {
        let account_id = self
            .connection_verifier
            .verify_jira(&setup_credentials, self.debug)
            .await?;
        let credentials = setup_credentials.to_credentials(account_id);
        self.connection_verifier
            .verify_tempo(&credentials, self.debug)
            .await?;

        let mut config = Config::load(&self.path)?;
        config.tempo_token = Some(credentials.tempo_token);
        config.account_id = Some(credentials.account_id);
        config.atlassian_user_email = Some(credentials.atlassian_user_email);
        config.atlassian_token = Some(credentials.atlassian_token);
        config.hostname = Some(credentials.hostname);
        config.save(&self.path)?;

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

fn required_setup_environment(name: &str) -> Result<String, CliError> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        Ok(_) | Err(env::VarError::NotPresent) => Err(CliError::InvalidInput(format!(
            "{name} must be set and non-empty for `drag setup --from-env`"
        ))),
        Err(env::VarError::NotUnicode(_)) => Err(CliError::InvalidInput(format!(
            "{name} must contain valid Unicode for `drag setup --from-env`"
        ))),
    }
}

fn prompt_credentials() -> Result<Credentials, CliError> {
    fn prompt(label: &str) -> Result<String, CliError> {
        eprint!("{label}: ");
        io::stderr().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let value = input.trim().to_owned();
        if value.is_empty() {
            Err(CliError::InvalidInput(format!("{label} must not be empty")))
        } else {
            Ok(value)
        }
    }
    let hostname_input = prompt("Atlassian hostname (yourcompany.atlassian.net)")?;
    let hostname = hostname_input
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_owned();
    if hostname.is_empty() || hostname.contains(['/', '?', '#']) {
        return Err(CliError::InvalidInput(
            "Atlassian hostname must not contain a path, query, or fragment".to_owned(),
        ));
    }
    let account_id = prompt("Atlassian account ID")?;
    let atlassian_user_email = prompt("Atlassian email")?;
    let atlassian_token = rpassword::prompt_password("Atlassian API token: ")?;
    let tempo_token = rpassword::prompt_password("Tempo API token: ")?;
    if atlassian_token.is_empty() || tempo_token.is_empty() {
        return Err(CliError::InvalidInput(
            "tokens must not be empty".to_owned(),
        ));
    }
    Ok(Credentials {
        tempo_token,
        account_id,
        atlassian_user_email,
        atlassian_token,
        hostname,
    })
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
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use drag::tracker::Tracker;
    use tempfile::TempDir;

    use super::{
        normalize_jira_site, App, Config, ConnectionVerifier, Credentials, SetupCredentials,
        VerificationFuture,
    };
    use crate::CliError;

    struct FakeVerifier {
        jira_error: Option<String>,
        tempo_error: Option<String>,
        tempo_accounts: Arc<Mutex<Vec<String>>>,
        config_update: Option<(PathBuf, Config)>,
    }

    impl ConnectionVerifier for FakeVerifier {
        fn verify_jira<'a>(
            &'a self,
            _credentials: &'a SetupCredentials,
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
            credentials: &'a Credentials,
            _debug: bool,
        ) -> VerificationFuture<'a, ()> {
            let account_id = credentials.account_id.clone();
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
