use std::io::{self, Read};
use std::path::Path;

use chrono::{DateTime, NaiveDate};
use chrono_tz::Tz;
use drag::models::{AddWorklogRequest, Worklog, WorklogEntity};
use drag::time::{
    clock_interval, format_duration, parse_clock, parse_duration_or_interval, select_date,
};
use serde_json::json;
use url::Url;

use crate::api::ApiClient;
use crate::cli::{LogArgs, LogInput};
use crate::config::{Config, Credentials};
use crate::{CliError, Rendered};

pub(crate) trait LogGateway: Send + Sync {
    async fn resolve_issue_id(&self, issue_key: &str) -> Result<String, CliError>;
    async fn create_worklog(&self, request: AddWorklogRequest) -> Result<WorklogEntity, CliError>;
}

pub(crate) struct ApiLogGateway {
    api: ApiClient,
}

impl ApiLogGateway {
    pub(crate) fn new(credentials: Credentials, debug: bool) -> Result<Self, CliError> {
        Ok(Self {
            api: ApiClient::new(credentials, debug)?,
        })
    }
}

impl LogGateway for ApiLogGateway {
    async fn resolve_issue_id(&self, issue_key: &str) -> Result<String, CliError> {
        self.api.get_issue_id(issue_key).await
    }

    async fn create_worklog(&self, request: AddWorklogRequest) -> Result<WorklogEntity, CliError> {
        self.api.add_worklog(request).await
    }
}

pub(crate) async fn run<G>(
    config_path: &Path,
    now: DateTime<Tz>,
    args: LogArgs,
    make_gateway: impl FnOnce(Credentials) -> Result<G, CliError>,
) -> Result<Rendered, CliError>
where
    G: LogGateway,
{
    let input = log_input(args)?;
    let config = Config::load(config_path)?;
    let credentials = config.credentials()?;
    let mut request = build_log_request(&config, &credentials, &input.value, now)?;
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
    let gateway = make_gateway(credentials)?;
    request.issue_id = gateway.resolve_issue_id(&issue_key).await?;
    let entity = gateway.create_worklog(request).await?;
    let worklog = to_worklog(entity, issue_key, now.timezone())?;
    Ok(Rendered::new(
        serde_json::to_value(&worklog)?,
        format!(
            "Successfully logged {} to {}, type `drag d {}` to undo.",
            worklog.duration, worklog.issue_key, worklog.id
        ),
    ))
}

#[derive(Clone, Copy)]
enum RemainingEstimateSyntax {
    DurationOnly,
    DurationOrInterval,
}

fn build_log_request(
    config: &Config,
    credentials: &Credentials,
    input: &LogInput,
    now: DateTime<Tz>,
) -> Result<AddWorklogRequest, CliError> {
    build_add_request_with_syntax(
        config,
        credentials,
        input,
        now,
        RemainingEstimateSyntax::DurationOnly,
    )
}

pub(crate) fn build_add_request(
    config: &Config,
    credentials: &Credentials,
    input: &LogInput,
    now: DateTime<Tz>,
) -> Result<AddWorklogRequest, CliError> {
    build_add_request_with_syntax(
        config,
        credentials,
        input,
        now,
        RemainingEstimateSyntax::DurationOrInterval,
    )
}

fn build_add_request_with_syntax(
    config: &Config,
    credentials: &Credentials,
    input: &LogInput,
    now: DateTime<Tz>,
    remaining_estimate_syntax: RemainingEstimateSyntax,
) -> Result<AddWorklogRequest, CliError> {
    let selected = select_date(now, input.when.as_deref())?;
    let parsed =
        parse_duration_or_interval(&input.duration_or_interval, selected.date, now.timezone())?;
    if parsed.seconds <= 0 {
        return Err(drag::Error::NonPositiveDuration.into());
    }
    let start = if let Some(start) = parsed.start_time {
        start
    } else if let Some(start) = &input.start {
        parse_clock(start).ok_or_else(|| drag::Error::InvalidTime(start.clone()))?
    } else {
        selected.default_start_time
    };
    let remaining_estimate_seconds = input
        .remaining_estimate
        .as_deref()
        .map(|remaining| {
            let parsed = parse_duration_or_interval(remaining, selected.date, now.timezone())?;
            if matches!(
                remaining_estimate_syntax,
                RemainingEstimateSyntax::DurationOnly
            ) && parsed.start_time.is_some()
            {
                return Err(drag::Error::InvalidDuration(remaining.to_owned()));
            }
            Ok(parsed.seconds)
        })
        .transpose()?;
    let issue_key = config
        .resolve_issue(&input.issue_key_or_alias)
        .to_uppercase();
    // The issue ID is filled by the async caller; this marker is replaced before upload.
    Ok(AddWorklogRequest {
        issue_id: format!("<resolved from {issue_key}>"),
        time_spent_seconds: parsed.seconds,
        start_date: selected.date.to_string(),
        start_time: start.format("%H:%M:%S").to_string(),
        description: input.description.clone(),
        remaining_estimate_seconds,
        author_account_id: Some(credentials.account_id.clone()),
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

pub(crate) fn to_worklog(
    entity: WorklogEntity,
    issue_key: String,
    timezone: Tz,
) -> Result<Worklog, CliError> {
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use chrono::TimeZone;
    use drag::models::{Author, Issue};
    use tempfile::TempDir;

    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    enum Operation {
        ResolveIssue(String),
        CreateWorklog(AddWorklogRequest),
    }

    struct FakeLogGateway {
        operations: Arc<Mutex<Vec<Operation>>>,
    }

    struct UnusedLogGateway;

    impl LogGateway for FakeLogGateway {
        async fn resolve_issue_id(&self, issue_key: &str) -> Result<String, CliError> {
            self.operations
                .lock()
                .map_err(|_| CliError::Api("test operation lock was poisoned".to_owned()))?
                .push(Operation::ResolveIssue(issue_key.to_owned()));
            Ok("10001".to_owned())
        }

        async fn create_worklog(
            &self,
            request: AddWorklogRequest,
        ) -> Result<WorklogEntity, CliError> {
            self.operations
                .lock()
                .map_err(|_| CliError::Api("test operation lock was poisoned".to_owned()))?
                .push(Operation::CreateWorklog(request.clone()));
            Ok(WorklogEntity {
                tempo_worklog_id: "751393".to_owned(),
                start_date: request.start_date,
                start_time: request.start_time,
                author: Author {
                    account_id: "account-1".to_owned(),
                },
                issue: Issue {
                    self_url: "https://example.atlassian.net/rest/api/3/issue/10001".to_owned(),
                    id: request.issue_id,
                },
                description: request.description.unwrap_or_default(),
                time_spent_seconds: request.time_spent_seconds,
            })
        }
    }

    impl LogGateway for UnusedLogGateway {
        async fn resolve_issue_id(&self, _issue_key: &str) -> Result<String, CliError> {
            Err(CliError::Api(
                "unused test gateway resolved an issue".to_owned(),
            ))
        }

        async fn create_worklog(
            &self,
            _request: AddWorklogRequest,
        ) -> Result<WorklogEntity, CliError> {
            Err(CliError::Api(
                "unused test gateway created a worklog".to_owned(),
            ))
        }
    }

    fn configured_file(directory: &TempDir) -> Result<PathBuf, CliError> {
        configured_file_with_aliases(directory, BTreeMap::new())
    }

    fn configured_file_with_aliases(
        directory: &TempDir,
        aliases: BTreeMap<String, String>,
    ) -> Result<PathBuf, CliError> {
        let path = directory.path().join("config.json");
        Config {
            tempo_token: Some("tempo-secret".to_owned()),
            account_id: Some("account-1".to_owned()),
            atlassian_user_email: Some("person@example.com".to_owned()),
            atlassian_token: Some("atlassian-secret".to_owned()),
            hostname: Some("example.atlassian.net".to_owned()),
            aliases,
            ..Config::default()
        }
        .save(&path)?;
        Ok(path)
    }

    fn fixed_now() -> Result<DateTime<Tz>, CliError> {
        chrono_tz::Europe::Warsaw
            .with_ymd_and_hms(2026, 7, 14, 12, 30, 0)
            .single()
            .ok_or_else(|| CliError::InvalidInput("invalid test date".to_owned()))
    }

    fn log_args(duration: &str) -> LogArgs {
        LogArgs {
            issue_key_or_alias: Some("abc-1".to_owned()),
            duration_or_interval: Some(duration.to_owned()),
            when: None,
            description: None,
            start: None,
            remaining_estimate: None,
            json: None,
            dry_run: false,
        }
    }

    fn reject_gateway_creation(_credentials: Credentials) -> Result<UnusedLogGateway, CliError> {
        Err(CliError::Api(
            "log gateway was unexpectedly created".to_owned(),
        ))
    }

    fn require_error(
        result: Result<Rendered, CliError>,
        expected: &str,
    ) -> Result<CliError, CliError> {
        result
            .err()
            .ok_or_else(|| CliError::Api(format!("expected {expected}")))
    }

    async fn preview(
        path: &Path,
        now: DateTime<Tz>,
        mut args: LogArgs,
    ) -> Result<Rendered, CliError> {
        args.dry_run = true;
        run(path, now, args, reject_gateway_creation).await
    }

    #[tokio::test]
    async fn duration_worklog_resolves_issue_before_creation_and_preserves_output(
    ) -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let now = fixed_now()?;
        let operations = Arc::new(Mutex::new(Vec::new()));
        let fake = FakeLogGateway {
            operations: Arc::clone(&operations),
        };

        let rendered = run(
            &path,
            now,
            LogArgs {
                issue_key_or_alias: Some("abc-1".to_owned()),
                duration_or_interval: Some("1h15m".to_owned()),
                when: None,
                description: Some("review".to_owned()),
                start: None,
                remaining_estimate: None,
                json: None,
                dry_run: false,
            },
            |_| Ok(fake),
        )
        .await?;

        let operations = operations
            .lock()
            .map_err(|_| CliError::Api("test operation lock was poisoned".to_owned()))?;
        assert_eq!(
            *operations,
            [
                Operation::ResolveIssue("ABC-1".to_owned()),
                Operation::CreateWorklog(AddWorklogRequest {
                    issue_id: "10001".to_owned(),
                    time_spent_seconds: 4_500,
                    start_date: "2026-07-14".to_owned(),
                    start_time: "12:30:00".to_owned(),
                    description: Some("review".to_owned()),
                    remaining_estimate_seconds: None,
                    author_account_id: Some("account-1".to_owned()),
                }),
            ]
        );
        assert_eq!(
            rendered.data,
            json!({
                "id": "751393",
                "interval": {"startTime": "12:30", "endTime": "13:45"},
                "issueId": "10001",
                "issueKey": "ABC-1",
                "duration": "1h15m",
                "description": "review",
                "link": "https://example.atlassian.net/browse/ABC-1"
            })
        );
        assert_eq!(
            rendered.human,
            "Successfully logged 1h15m to ABC-1, type `drag d 751393` to undo."
        );
        Ok(())
    }

    #[tokio::test]
    async fn duration_forms_are_normalized_to_elapsed_seconds() -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let now = fixed_now()?;

        for (duration, expected) in [("15m", 900), ("1h", 3_600), ("1h15m", 4_500)] {
            let rendered = preview(&path, now, log_args(duration)).await?;
            assert_eq!(
                rendered.data["request"]["timeSpentSeconds"], expected,
                "unexpected elapsed seconds for {duration}"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn zero_duration_fails_before_gateway_creation() -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;

        let error = require_error(
            run(&path, fixed_now()?, log_args("0m"), reject_gateway_creation).await,
            "zero duration to fail",
        )?;

        assert!(matches!(
            error,
            CliError::Core(drag::Error::NonPositiveDuration)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn malformed_duration_fails_before_gateway_creation() -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;

        let error = require_error(
            run(
                &path,
                fixed_now()?,
                log_args("nonsense"),
                reject_gateway_creation,
            )
            .await,
            "malformed duration to fail",
        )?;

        assert!(matches!(
            error,
            CliError::Core(drag::Error::InvalidDuration(value)) if value == "nonsense"
        ));
        Ok(())
    }

    #[tokio::test]
    async fn configured_alias_is_resolved_before_issue_key_normalization() -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file_with_aliases(
            &directory,
            BTreeMap::from([("focus".to_owned(), "team-7".to_owned())]),
        )?;
        let mut args = log_args("30m");
        args.issue_key_or_alias = Some("focus".to_owned());

        let rendered = preview(&path, fixed_now()?, args).await?;

        assert_eq!(rendered.data["issueKey"], "TEAM-7");
        Ok(())
    }

    #[tokio::test]
    async fn unmatched_alias_input_is_normalized_as_an_issue_key() -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;

        let rendered = preview(&path, fixed_now()?, log_args("30m")).await?;

        assert_eq!(rendered.data["issueKey"], "ABC-1");
        Ok(())
    }

    #[tokio::test]
    async fn omitted_date_uses_today_and_current_local_time() -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;

        let rendered = preview(&path, fixed_now()?, log_args("30m")).await?;

        assert_eq!(
            rendered.data["request"],
            json!({
                "issueId": "<resolved from ABC-1>",
                "timeSpentSeconds": 1_800,
                "startDate": "2026-07-14",
                "startTime": "12:30:00",
                "authorAccountId": "account-1"
            })
        );
        Ok(())
    }

    #[tokio::test]
    async fn explicit_and_relative_dates_select_expected_local_days() -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let now = fixed_now()?;

        for (selector, expected) in [
            ("2026-07-01", "2026-07-01"),
            ("y", "2026-07-13"),
            ("yesterday", "2026-07-13"),
            ("t-2", "2026-07-12"),
            ("today+1", "2026-07-15"),
        ] {
            let mut args = log_args("30m");
            args.when = Some(selector.to_owned());
            let rendered = preview(&path, now, args).await?;
            assert_eq!(
                rendered.data["request"]["startDate"], expected,
                "unexpected date for {selector}"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn explicit_date_without_start_uses_midnight() -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let mut args = log_args("30m");
        args.when = Some("2026-07-01".to_owned());

        let rendered = preview(&path, fixed_now()?, args).await?;

        assert_eq!(rendered.data["request"]["startTime"], "00:00:00");
        Ok(())
    }

    #[tokio::test]
    async fn explicit_start_is_normalized_for_duration_input() -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let mut args = log_args("30m");
        args.start = Some("9:05".to_owned());

        let rendered = preview(&path, fixed_now()?, args).await?;

        assert_eq!(rendered.data["request"]["startTime"], "09:05:00");
        Ok(())
    }

    #[tokio::test]
    async fn invalid_start_fails_before_gateway_creation() -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let mut args = log_args("30m");
        args.start = Some("25:00".to_owned());

        let error = require_error(
            run(&path, fixed_now()?, args, reject_gateway_creation).await,
            "invalid start to fail",
        )?;

        assert!(matches!(
            error,
            CliError::Core(drag::Error::InvalidTime(value)) if value == "25:00"
        ));
        Ok(())
    }

    #[tokio::test]
    async fn invalid_and_overflowing_dates_fail_before_gateway_creation() -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let now = fixed_now()?;

        for selector in ["not-a-date", "t+9223372036854775807"] {
            let mut args = log_args("30m");
            args.when = Some(selector.to_owned());
            let error = require_error(
                run(&path, now, args, reject_gateway_creation).await,
                "invalid date to fail",
            )?;
            assert!(
                matches!(error, CliError::Core(drag::Error::InvalidDate(ref value)) if value == selector),
                "unexpected error for {selector}: {error}"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn description_and_remaining_estimate_are_preserved() -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let mut args = log_args("30m");
        args.description = Some("review with the team".to_owned());
        args.remaining_estimate = Some("2h15m".to_owned());

        let rendered = preview(&path, fixed_now()?, args).await?;

        assert_eq!(
            rendered.data["request"],
            json!({
                "issueId": "<resolved from ABC-1>",
                "timeSpentSeconds": 1_800,
                "startDate": "2026-07-14",
                "startTime": "12:30:00",
                "description": "review with the team",
                "remainingEstimateSeconds": 8_100,
                "authorAccountId": "account-1"
            })
        );
        Ok(())
    }

    #[tokio::test]
    async fn interval_remaining_estimate_fails_before_gateway_creation() -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let mut args = log_args("30m");
        args.remaining_estimate = Some("11-12".to_owned());

        let error = require_error(
            run(&path, fixed_now()?, args, reject_gateway_creation).await,
            "interval remaining estimate to fail",
        )?;

        assert!(matches!(
            error,
            CliError::Core(drag::Error::InvalidDuration(value)) if value == "11-12"
        ));
        Ok(())
    }

    #[tokio::test]
    async fn malformed_remaining_estimate_fails_before_gateway_creation() -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let mut args = log_args("30m");
        args.remaining_estimate = Some("soon".to_owned());

        let error = require_error(
            run(&path, fixed_now()?, args, reject_gateway_creation).await,
            "malformed remaining estimate to fail",
        )?;

        assert!(matches!(
            error,
            CliError::Core(drag::Error::InvalidDuration(value)) if value == "soon"
        ));
        Ok(())
    }

    #[tokio::test]
    async fn dry_run_returns_normalized_preview_without_creating_gateway() -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file_with_aliases(
            &directory,
            BTreeMap::from([("focus".to_owned(), "team-7".to_owned())]),
        )?;
        let mut args = log_args("30m");
        args.issue_key_or_alias = Some("focus".to_owned());
        args.when = Some("2026-07-01".to_owned());
        args.start = Some("9:05".to_owned());
        args.description = Some("review".to_owned());
        args.remaining_estimate = Some("2h".to_owned());

        let rendered = preview(&path, fixed_now()?, args).await?;

        assert_eq!(
            rendered.data,
            json!({
                "dryRun": true,
                "issueKey": "TEAM-7",
                "request": {
                    "issueId": "<resolved from TEAM-7>",
                    "timeSpentSeconds": 1_800,
                    "startDate": "2026-07-01",
                    "startTime": "09:05:00",
                    "description": "review",
                    "remainingEstimateSeconds": 7_200,
                    "authorAccountId": "account-1"
                }
            })
        );
        assert_eq!(rendered.human, "Would log 30m to focus.");
        Ok(())
    }
}
