use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use chrono::{DateTime, NaiveDate};
use chrono_tz::Tz;
use comfy_table::{presets::UTF8_FULL, ContentArrangement, Table};
use drag::models::{ScheduleEntity, Worklog, WorklogEntity};
use drag::schedule::{create_schedule_details, ScheduleDetails};
use drag::time::{clock_interval, format_duration, month_bounds, select_date};
use serde_json::json;
use url::Url;

use crate::api::ApiClient;
use crate::cli::ListArgs;
use crate::config::{Config, Credentials};
use crate::{CliError, Rendered};

type ListFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, CliError>> + Send + 'a>>;

pub(crate) trait ListDataSource: Send + Sync {
    fn worklogs<'a>(&'a self, from: &'a str, to: &'a str) -> ListFuture<'a, Vec<WorklogEntity>>;
    fn schedule<'a>(&'a self, from: &'a str, to: &'a str) -> ListFuture<'a, Vec<ScheduleEntity>>;
    fn issue_key<'a>(&'a self, issue_id: &'a str) -> ListFuture<'a, String>;
}

pub(crate) struct ApiListDataSource {
    api: ApiClient,
}

impl ApiListDataSource {
    pub(crate) fn new(credentials: Credentials, debug: bool) -> Result<Self, CliError> {
        Ok(Self {
            api: ApiClient::new(credentials, debug)?,
        })
    }
}

impl ListDataSource for ApiListDataSource {
    fn worklogs<'a>(&'a self, from: &'a str, to: &'a str) -> ListFuture<'a, Vec<WorklogEntity>> {
        Box::pin(self.api.get_worklogs(from, to))
    }

    fn schedule<'a>(&'a self, from: &'a str, to: &'a str) -> ListFuture<'a, Vec<ScheduleEntity>> {
        Box::pin(self.api.get_schedule(from, to))
    }

    fn issue_key<'a>(&'a self, issue_id: &'a str) -> ListFuture<'a, String> {
        Box::pin(self.api.get_issue_key(issue_id))
    }
}

pub(crate) async fn run(
    config_path: &Path,
    now: DateTime<Tz>,
    args: ListArgs,
    make_source: impl FnOnce(Credentials) -> Result<Box<dyn ListDataSource>, CliError>,
) -> Result<Rendered, CliError> {
    let config = Config::load(config_path)?;
    let credentials = config.credentials()?;
    let selected = select_date(now, args.when.as_deref())?;
    let source = make_source(credentials.clone())?;
    let (month_start, month_end) = month_bounds(selected.date);
    let month_start = month_start.to_string();
    let month_end = month_end.to_string();
    let (entities, schedule) = tokio::try_join!(
        source.worklogs(&month_start, &month_end),
        source.schedule(&month_start, &month_end)
    )?;
    let details = create_schedule_details(
        &entities,
        &schedule,
        selected.date,
        now.date_naive(),
        &credentials.account_id,
    );
    let selected_date = selected.date.to_string();
    let selected_entities: Vec<_> = entities
        .iter()
        .filter(|entity| {
            entity.author.account_id == credentials.account_id && entity.start_date == selected_date
        })
        .collect();
    let issue_ids: BTreeSet<_> = selected_entities
        .iter()
        .map(|entity| entity.issue.id.as_str())
        .collect();
    let mut issue_keys = BTreeMap::new();
    for issue_id in issue_ids {
        issue_keys.insert(issue_id, source.issue_key(issue_id).await?);
    }
    let worklogs = selected_entities
        .into_iter()
        .map(|entity| {
            let issue_key = issue_keys
                .get(entity.issue.id.as_str())
                .cloned()
                .ok_or_else(|| CliError::Api("Atlassian did not return an issue key".to_owned()))?;
            to_worklog(entity.clone(), issue_key, now.timezone())
        })
        .collect::<Result<Vec<_>, _>>()?;
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

fn to_worklog(entity: WorklogEntity, issue_key: String, timezone: Tz) -> Result<Worklog, CliError> {
    let date = NaiveDate::parse_from_str(&entity.start_date, "%Y-%m-%d")
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
            timezone,
        ),
        issue_id: entity.issue.id,
        duration: format_duration(entity.time_spent_seconds, false),
        description: entity.description,
        link: format!("https://{hostname}/browse/{issue_key}"),
        issue_key,
    })
}

fn worklogs_table(
    date: NaiveDate,
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
            issue_with_aliases(&worklog.issue_key, aliases),
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

fn issue_with_aliases(issue_key: &str, aliases: &BTreeMap<String, String>) -> String {
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
    format!("({truncated}{suffix}) {issue_key}")
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::TimeZone;
    use drag::models::{Author, Issue};
    use tempfile::TempDir;

    use super::*;

    #[derive(Default)]
    struct Requests {
        worklogs: Vec<(String, String)>,
        schedules: Vec<(String, String)>,
        issues: Vec<String>,
    }

    struct FakeListDataSource {
        worklogs: Vec<WorklogEntity>,
        schedule: Vec<ScheduleEntity>,
        requests: Arc<Mutex<Requests>>,
    }

    impl ListDataSource for FakeListDataSource {
        fn worklogs<'a>(
            &'a self,
            from: &'a str,
            to: &'a str,
        ) -> ListFuture<'a, Vec<WorklogEntity>> {
            self.requests
                .lock()
                .map(|mut requests| requests.worklogs.push((from.to_owned(), to.to_owned())))
                .ok();
            let worklogs = self.worklogs.clone();
            Box::pin(async move { Ok(worklogs) })
        }

        fn schedule<'a>(
            &'a self,
            from: &'a str,
            to: &'a str,
        ) -> ListFuture<'a, Vec<ScheduleEntity>> {
            self.requests
                .lock()
                .map(|mut requests| requests.schedules.push((from.to_owned(), to.to_owned())))
                .ok();
            let schedule = self.schedule.clone();
            Box::pin(async move { Ok(schedule) })
        }

        fn issue_key<'a>(&'a self, issue_id: &'a str) -> ListFuture<'a, String> {
            self.requests
                .lock()
                .map(|mut requests| requests.issues.push(issue_id.to_owned()))
                .ok();
            Box::pin(async move { Ok(format!("KEY-{issue_id}")) })
        }
    }

    fn configured_file(directory: &TempDir) -> Result<std::path::PathBuf, CliError> {
        let path = directory.path().join("config.json");
        Config {
            tempo_token: Some("tempo-secret".to_owned()),
            account_id: Some("me".to_owned()),
            atlassian_user_email: Some("me@example.com".to_owned()),
            atlassian_token: Some("jira-secret".to_owned()),
            hostname: Some("example.atlassian.net".to_owned()),
            ..Config::default()
        }
        .save(&path)?;
        Ok(path)
    }

    fn other_day_worklog() -> WorklogEntity {
        WorklogEntity {
            tempo_worklog_id: "1".to_owned(),
            start_date: "2026-07-13".to_owned(),
            start_time: "09:00:00".to_owned(),
            author: Author {
                account_id: "me".to_owned(),
            },
            issue: Issue {
                self_url: "https://example.atlassian.net/rest/api/3/issue/10".to_owned(),
                id: "10".to_owned(),
            },
            description: "not selected".to_owned(),
            time_spent_seconds: 3_600,
        }
    }

    #[tokio::test]
    async fn empty_selected_day_uses_month_data_without_jira_requests() -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let requests = Arc::new(Mutex::new(Requests::default()));
        let fake = FakeListDataSource {
            worklogs: vec![other_day_worklog()],
            schedule: vec![ScheduleEntity {
                date: "2026-07-14".to_owned(),
                required_seconds: 28_800,
                kind: "WORKING_DAY".to_owned(),
            }],
            requests: Arc::clone(&requests),
        };
        let now = chrono_tz::UTC
            .with_ymd_and_hms(2026, 7, 14, 12, 0, 0)
            .single()
            .ok_or_else(|| CliError::InvalidInput("invalid test date".to_owned()))?;

        let rendered = run(
            &path,
            now,
            ListArgs {
                when: None,
                verbose: false,
            },
            |_| Ok(Box::new(fake)),
        )
        .await?;

        assert_eq!(rendered.data["date"], "2026-07-14");
        assert_eq!(rendered.data["worklogs"], json!([]));
        assert_eq!(rendered.data["schedule"]["monthLoggedDuration"], "1h");
        assert_eq!(rendered.data["schedule"]["dayRequiredDuration"], "8h");
        assert_eq!(rendered.data["schedule"]["dayLoggedDuration"], "0h");
        assert!(rendered.human.contains("Tuesday, 2026-07-14"));
        assert!(rendered.human.contains("No worklogs"));
        assert!(rendered.human.contains("Required 8h, logged: 0h"));

        let requests = requests
            .lock()
            .map_err(|_| CliError::Api("test request lock was poisoned".to_owned()))?;
        assert_eq!(
            requests.worklogs,
            [("2026-07-01".to_owned(), "2026-07-31".to_owned())]
        );
        assert_eq!(requests.schedules, requests.worklogs);
        assert!(requests.issues.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn fully_empty_month_makes_no_jira_requests() -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let requests = Arc::new(Mutex::new(Requests::default()));
        let fake = FakeListDataSource {
            worklogs: Vec::new(),
            schedule: Vec::new(),
            requests: Arc::clone(&requests),
        };
        let now = chrono_tz::UTC
            .with_ymd_and_hms(2026, 7, 14, 12, 0, 0)
            .single()
            .ok_or_else(|| CliError::InvalidInput("invalid test date".to_owned()))?;

        let rendered = run(
            &path,
            now,
            ListArgs {
                when: Some("2026-07-01".to_owned()),
                verbose: false,
            },
            |_| Ok(Box::new(fake)),
        )
        .await?;

        assert_eq!(rendered.data["worklogs"], json!([]));
        assert!(rendered.human.contains("No worklogs"));
        let requests = requests
            .lock()
            .map_err(|_| CliError::Api("test request lock was poisoned".to_owned()))?;
        assert!(requests.issues.is_empty());
        Ok(())
    }
}
