use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::io::{self, Write};
use std::path::Path;
use std::pin::Pin;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, NaiveDate};
use chrono_tz::Tz;
use comfy_table::{presets::UTF8_FULL, ContentArrangement, Table};
use drag::field_selection::{project_list_result, ListField, ListFieldMask};
use drag::models::{ListPagination, ScheduleEntity, Worklog, WorklogEntity};
use drag::pagination::{PaginationPlan, DEFAULT_PAGE_LIMIT, DEFAULT_RECORD_LIMIT, HARD_PAGE_LIMIT};
use drag::schedule::{create_schedule_details, ScheduleAccumulator, ScheduleDetails};
use drag::time::{clock_interval, format_duration, month_bounds, select_date};
use serde::{Deserialize, Serialize};
use serde_json::json;
use url::Url;

use crate::api::{validate_tempo_continuation_input, ApiClient, WorklogPage};
use crate::cli::ListArgs;
use crate::config::{Config, Credentials};
use crate::output::escape_terminal_data;
use crate::{CliError, Rendered};

type ListFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, CliError>> + Send + 'a>>;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListContinuation {
    version: u8,
    selected_date: String,
    month_start: String,
    month_end: String,
    url: String,
    limit: Option<usize>,
    page_limit: u16,
    all_pages: bool,
}

fn encode_continuation(continuation: &ListContinuation) -> Result<String, CliError> {
    Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(continuation)?))
}

fn decode_continuation(value: &str) -> Result<ListContinuation, CliError> {
    let bytes = URL_SAFE_NO_PAD.decode(value).map_err(|_| {
        CliError::InvalidInput("continuation token is malformed or incompatible".to_owned())
    })?;
    serde_json::from_slice(&bytes).map_err(|_| {
        CliError::InvalidInput("continuation token is malformed or incompatible".to_owned())
    })
}

fn requested_plan(
    args: &ListArgs,
    continuation: Option<&ListContinuation>,
) -> Result<PaginationPlan, CliError> {
    let Some(continuation) = continuation else {
        return if args.all_pages {
            Ok(PaginationPlan::all_pages())
        } else {
            PaginationPlan::bounded(
                args.limit.map_or(DEFAULT_RECORD_LIMIT, usize::from),
                args.page_limit.unwrap_or(DEFAULT_PAGE_LIMIT),
            )
            .map_err(Into::into)
        };
    };

    let plan = if continuation.all_pages {
        if continuation.limit.is_some() || continuation.page_limit != HARD_PAGE_LIMIT {
            return Err(incompatible_continuation_plan());
        }
        PaginationPlan::all_pages()
    } else {
        PaginationPlan::bounded(
            continuation
                .limit
                .ok_or_else(incompatible_continuation_plan)?,
            continuation.page_limit,
        )?
    };
    if (args.all_pages && !continuation.all_pages)
        || (continuation.all_pages && (args.limit.is_some() || args.page_limit.is_some()))
        || args
            .limit
            .is_some_and(|limit| Some(usize::from(limit)) != plan.record_limit())
        || args
            .page_limit
            .is_some_and(|page_limit| page_limit != plan.page_limit())
    {
        return Err(incompatible_continuation_plan());
    }
    Ok(plan)
}

fn incompatible_continuation_plan() -> CliError {
    CliError::InvalidInput(
        "explicit pagination options do not match the continuation token".to_owned(),
    )
}

pub(crate) trait ListDataSource: Send + Sync {
    fn worklogs<'a>(
        &'a self,
        from: &'a str,
        to: &'a str,
        plan: PaginationPlan,
        continue_from: Option<&'a str>,
    ) -> ListFuture<'a, WorklogPage>;
    fn worklog_page<'a>(
        &'a self,
        from: &'a str,
        to: &'a str,
        plan: PaginationPlan,
        continue_from: Option<&'a str>,
        _records_retrieved: usize,
    ) -> ListFuture<'a, WorklogPage>;
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
    fn worklogs<'a>(
        &'a self,
        from: &'a str,
        to: &'a str,
        plan: PaginationPlan,
        continue_from: Option<&'a str>,
    ) -> ListFuture<'a, WorklogPage> {
        Box::pin(self.api.get_worklogs_bounded(from, to, plan, continue_from))
    }

    fn worklog_page<'a>(
        &'a self,
        from: &'a str,
        to: &'a str,
        plan: PaginationPlan,
        continue_from: Option<&'a str>,
        records_retrieved: usize,
    ) -> ListFuture<'a, WorklogPage> {
        Box::pin(
            self.api
                .get_worklog_page(from, to, plan, continue_from, records_retrieved),
        )
    }

    fn schedule<'a>(&'a self, from: &'a str, to: &'a str) -> ListFuture<'a, Vec<ScheduleEntity>> {
        Box::pin(self.api.get_schedule(from, to))
    }

    fn issue_key<'a>(&'a self, issue_id: &'a str) -> ListFuture<'a, String> {
        Box::pin(self.api.get_issue_key(issue_id))
    }
}

struct PreparedList {
    fields: Option<ListFieldMask>,
    selected_date: NaiveDate,
    today: NaiveDate,
    details: ScheduleDetails,
    pagination: ListPagination,
    selected_entities: Vec<WorklogEntity>,
    aliases: BTreeMap<String, String>,
    source: Box<dyn ListDataSource>,
    timezone: Tz,
}

/// Completed list data shared by human and structured presentations.
#[derive(Debug, Clone)]
pub(crate) struct ListReport {
    selected_date: NaiveDate,
    today: NaiveDate,
    worklogs: Vec<Worklog>,
    details: ScheduleDetails,
    pagination: ListPagination,
    aliases: BTreeMap<String, String>,
    verbose: bool,
    fields: Option<ListFieldMask>,
}

impl ListReport {
    pub(crate) fn new(
        selected_date: NaiveDate,
        worklogs: Vec<Worklog>,
        details: ScheduleDetails,
        pagination: ListPagination,
        aliases: BTreeMap<String, String>,
        verbose: bool,
    ) -> Self {
        Self {
            selected_date,
            today: selected_date,
            worklogs,
            details,
            pagination,
            aliases,
            verbose,
            fields: None,
        }
    }

    fn with_fields(mut self, fields: Option<ListFieldMask>) -> Self {
        self.fields = fields;
        self
    }

    pub(crate) fn with_today(mut self, today: NaiveDate) -> Self {
        self.today = today;
        self
    }

    pub(crate) fn worklogs(&self) -> &[Worklog] {
        &self.worklogs
    }

    pub(crate) fn selected_date(&self) -> NaiveDate {
        self.selected_date
    }

    pub(crate) fn today(&self) -> NaiveDate {
        self.today
    }

    pub(crate) fn schedule(&self) -> &ScheduleDetails {
        &self.details
    }

    pub(crate) fn pagination(&self) -> &ListPagination {
        &self.pagination
    }

    pub(crate) fn verbose(&self) -> bool {
        self.verbose
    }

    pub(crate) fn issue_label(&self, worklog: &Worklog) -> String {
        issue_with_aliases(&worklog.issue_key, &self.aliases)
    }

    fn plain_text(&self) -> String {
        let mut human = worklogs_table(self);
        if !self.pagination().totals_complete {
            if self.pagination().next.is_some() {
                human.push_str(
                    "\nMore worklogs are available; use JSON pagination metadata to continue.",
                );
            }
            human.push_str("\nTotals reflect this bounded segment.");
        }
        human
    }

    fn structured_data(&self, fields: Option<&ListFieldMask>) -> serde_json::Value {
        fields.map_or_else(
            || {
                json!({
                    "date": self.selected_date(),
                    "worklogs": self.worklogs(),
                    "schedule": self.schedule(),
                    "pagination": self.pagination(),
                })
            },
            |mask| {
                project_list_result(
                    self.selected_date(),
                    self.worklogs(),
                    self.schedule(),
                    self.pagination(),
                    mask,
                )
            },
        )
    }

    pub(crate) fn rendered(&self) -> Rendered {
        Rendered::new(
            self.structured_data(self.fields.as_ref()),
            self.plain_text(),
        )
    }
}

struct ListSelection {
    fields: Option<ListFieldMask>,
    selected_date: NaiveDate,
    month_start: String,
    month_end: String,
    continuation: Option<ListContinuation>,
    plan: PaginationPlan,
}

fn list_selection(now: DateTime<Tz>, args: &ListArgs) -> Result<ListSelection, CliError> {
    let fields = args
        .fields
        .as_deref()
        .map(ListFieldMask::parse)
        .transpose()?;
    let selected = select_date(now, args.when.as_deref())?;
    let (month_start, month_end) = month_bounds(selected.date);
    let month_start = month_start.to_string();
    let month_end = month_end.to_string();
    let continuation = args
        .continue_from
        .as_deref()
        .map(decode_continuation)
        .transpose()?;
    let plan = requested_plan(args, continuation.as_ref())?;
    if let Some(continuation) = &continuation {
        if continuation.version != 1
            || continuation.selected_date != selected.date.to_string()
            || continuation.month_start != month_start
            || continuation.month_end != month_end
        {
            return Err(CliError::InvalidInput(
                "continuation token does not match the selected date".to_owned(),
            ));
        }
        validate_tempo_continuation_input(&continuation.url, &month_start, &month_end, plan)?;
    }
    Ok(ListSelection {
        fields,
        selected_date: selected.date,
        month_start,
        month_end,
        continuation,
        plan,
    })
}

async fn prepare(
    config_path: &Path,
    now: DateTime<Tz>,
    args: &ListArgs,
    make_source: impl FnOnce(Credentials) -> Result<Box<dyn ListDataSource>, CliError>,
) -> Result<PreparedList, CliError> {
    let selection = list_selection(now, args)?;
    let config = Config::load(config_path)?;
    let credentials = config.credentials()?;
    let aliases = config.aliases;
    let source = make_source(credentials.clone())?;
    let (page, schedule) = tokio::try_join!(
        source.worklogs(
            &selection.month_start,
            &selection.month_end,
            selection.plan,
            selection
                .continuation
                .as_ref()
                .map(|value| value.url.as_str())
        ),
        source.schedule(&selection.month_start, &selection.month_end)
    )?;
    let WorklogPage {
        results: entities,
        next,
        pages_retrieved,
    } = page;
    let details = create_schedule_details(
        &entities,
        &schedule,
        selection.selected_date,
        now.date_naive(),
        &credentials.account_id,
    );
    let selected_date = selection.selected_date.to_string();
    let records_retrieved = entities.len();
    let selected_entities: Vec<_> = entities
        .into_iter()
        .filter(|entity| {
            entity.author.account_id == credentials.account_id && entity.start_date == selected_date
        })
        .collect();
    let complete = next.is_none();
    let totals_complete = selection.continuation.is_none() && complete;
    let records_returned = selected_entities.len();
    let next = next
        .map(|url| {
            encode_continuation(&ListContinuation {
                version: 1,
                selected_date: selection.selected_date.to_string(),
                month_start: selection.month_start.clone(),
                month_end: selection.month_end.clone(),
                url,
                limit: selection.plan.record_limit(),
                page_limit: selection.plan.page_limit(),
                all_pages: selection.plan.is_all_pages(),
            })
        })
        .transpose()?;
    let pagination = ListPagination {
        selected_date: selection.selected_date.to_string(),
        month_start: selection.month_start,
        month_end: selection.month_end,
        limit: selection.plan.record_limit(),
        page_limit: selection.plan.page_limit(),
        all_pages: selection.plan.is_all_pages(),
        pages_retrieved,
        records_retrieved,
        records_returned,
        next,
        complete,
        totals_complete,
    };

    Ok(PreparedList {
        fields: selection.fields,
        selected_date: selection.selected_date,
        today: now.date_naive(),
        details,
        pagination,
        selected_entities,
        aliases,
        source,
        timezone: now.timezone(),
    })
}

#[cfg(test)]
pub(crate) async fn run(
    config_path: &Path,
    now: DateTime<Tz>,
    args: ListArgs,
    make_source: impl FnOnce(Credentials) -> Result<Box<dyn ListDataSource>, CliError>,
) -> Result<Rendered, CliError> {
    Ok(run_report(config_path, now, args, make_source)
        .await?
        .rendered())
}

pub(crate) async fn run_report(
    config_path: &Path,
    now: DateTime<Tz>,
    args: ListArgs,
    make_source: impl FnOnce(Credentials) -> Result<Box<dyn ListDataSource>, CliError>,
) -> Result<ListReport, CliError> {
    let prepared = prepare(config_path, now, &args, make_source).await?;
    let issue_ids: BTreeSet<_> = prepared
        .selected_entities
        .iter()
        .map(|entity| entity.issue.id.clone())
        .collect();
    let mut issue_keys: BTreeMap<String, String> = BTreeMap::new();
    for issue_id in issue_ids {
        let issue_key = prepared.source.issue_key(&issue_id).await?;
        issue_keys.insert(issue_id, issue_key);
    }
    let worklogs = prepared
        .selected_entities
        .into_iter()
        .map(|entity| {
            let issue_key = issue_keys
                .get(entity.issue.id.as_str())
                .cloned()
                .ok_or_else(|| CliError::Api("Atlassian did not return an issue key".to_owned()))?;
            to_worklog(entity, issue_key, prepared.timezone)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ListReport::new(
        prepared.selected_date,
        worklogs,
        prepared.details,
        prepared.pagination,
        prepared.aliases,
        args.verbose,
    )
    .with_today(prepared.today)
    .with_fields(prepared.fields))
}

pub(crate) async fn run_stream(
    config_path: &Path,
    now: DateTime<Tz>,
    args: ListArgs,
    make_source: impl FnOnce(Credentials) -> Result<Box<dyn ListDataSource>, CliError>,
    writer: &mut impl Write,
) -> Result<(), CliError> {
    let selection = list_selection(now, &args)?;
    let config = Config::load(config_path)?;
    let credentials = config.credentials()?;
    let source = make_source(credentials.clone())?;
    let schedule = source
        .schedule(&selection.month_start, &selection.month_end)
        .await?;
    let mut schedule_totals = ScheduleAccumulator::new(
        &schedule,
        selection.selected_date,
        now.date_naive(),
        &credentials.account_id,
    );
    let selected_date = selection.selected_date.to_string();
    let mut continuation = selection
        .continuation
        .as_ref()
        .map(|value| value.url.clone());
    let mut pages_retrieved = 0_u16;
    let mut records_retrieved = 0_usize;
    let mut records_returned = 0_usize;
    let mut issue_keys: BTreeMap<String, String> = BTreeMap::new();
    let (next, complete) = loop {
        let page = source
            .worklog_page(
                &selection.month_start,
                &selection.month_end,
                selection.plan,
                continuation.as_deref(),
                records_retrieved,
            )
            .await?;
        pages_retrieved += page.pages_retrieved;
        records_retrieved += page.results.len();
        schedule_totals.add_worklogs(&page.results);

        for entity in page.results.into_iter().filter(|entity| {
            entity.author.account_id == credentials.account_id && entity.start_date == selected_date
        }) {
            records_returned += 1;
            if let Some(worklog) = stream_worklog_value(
                entity,
                selection.fields.as_ref(),
                now.timezone(),
                source.as_ref(),
                &mut issue_keys,
            )
            .await?
            {
                if !write_stream_event(writer, &json!({"kind": "worklog", "worklog": worklog}))? {
                    return Ok(());
                }
            }
        }

        let Some(next) = page.next else {
            break (None, true);
        };
        if !selection
            .plan
            .should_follow(pages_retrieved, records_retrieved)
        {
            if selection.plan.is_all_pages() && pages_retrieved == HARD_PAGE_LIMIT {
                return Err(CliError::Api(
                    "Tempo pagination exceeded the 100-page safety limit".to_owned(),
                ));
            }
            break (Some(next), false);
        }
        continuation = Some(next);
    };

    let totals_complete = selection.continuation.is_none() && complete;
    let next = next
        .map(|url| {
            encode_continuation(&ListContinuation {
                version: 1,
                selected_date: selection.selected_date.to_string(),
                month_start: selection.month_start.clone(),
                month_end: selection.month_end.clone(),
                url,
                limit: selection.plan.record_limit(),
                page_limit: selection.plan.page_limit(),
                all_pages: selection.plan.is_all_pages(),
            })
        })
        .transpose()?;
    let details = schedule_totals.finish();
    let pagination = ListPagination {
        selected_date: selection.selected_date.to_string(),
        month_start: selection.month_start,
        month_end: selection.month_end,
        limit: selection.plan.record_limit(),
        page_limit: selection.plan.page_limit(),
        all_pages: selection.plan.is_all_pages(),
        pages_retrieved,
        records_retrieved,
        records_returned,
        next,
        complete,
        totals_complete,
    };
    let projected = selection
        .fields
        .as_ref()
        .map(|mask| project_list_result(selection.selected_date, &[], &details, &pagination, mask));
    let mut summary = serde_json::Map::from_iter([("kind".to_owned(), json!("summary"))]);
    if let Some(projected) = &projected {
        if let Some(date) = projected.get("date") {
            summary.insert("date".to_owned(), date.clone());
        }
        if let Some(schedule) = projected.get("schedule") {
            summary.insert("schedule".to_owned(), schedule.clone());
        }
    } else {
        summary.insert("date".to_owned(), json!(selection.selected_date));
        summary.insert("schedule".to_owned(), json!(details));
    }
    if !write_stream_event(writer, &serde_json::Value::Object(summary))? {
        return Ok(());
    }

    let mut terminal = serde_json::Map::from_iter([("kind".to_owned(), json!("pagination"))]);
    if let Some(projected) = &projected {
        if let Some(pagination) = projected.get("pagination") {
            terminal.insert("pagination".to_owned(), pagination.clone());
        }
    } else {
        terminal.insert("pagination".to_owned(), json!(pagination));
    }
    let _ = write_stream_event(writer, &serde_json::Value::Object(terminal))?;
    Ok(())
}

async fn stream_worklog_value(
    entity: WorklogEntity,
    fields: Option<&ListFieldMask>,
    timezone: Tz,
    source: &dyn ListDataSource,
    issue_keys: &mut BTreeMap<String, String>,
) -> Result<Option<serde_json::Value>, CliError> {
    let Some(fields) = fields else {
        let issue_key = resolved_issue_key(source, issue_keys, &entity.issue.id).await?;
        return Ok(Some(json!(to_worklog(entity, issue_key, timezone)?)));
    };
    if !fields.selects_worklogs() {
        return Ok(None);
    }

    let needs_issue_key =
        fields.includes(ListField::WorklogIssueKey) || fields.includes(ListField::WorklogLink);
    let issue_key = if needs_issue_key {
        Some(resolved_issue_key(source, issue_keys, &entity.issue.id).await?)
    } else {
        None
    };
    let needs_interval = fields.includes(ListField::WorklogIntervalStartTime)
        || fields.includes(ListField::WorklogIntervalEndTime);
    let interval = needs_interval
        .then(|| worklog_interval(&entity, timezone))
        .transpose()?;
    let link = if fields.includes(ListField::WorklogLink) {
        Some(worklog_link(
            &entity,
            issue_key
                .as_deref()
                .ok_or_else(|| CliError::Api("Atlassian did not return an issue key".to_owned()))?,
        )?)
    } else {
        None
    };

    let mut worklog = serde_json::Map::new();
    if fields.includes(ListField::WorklogId) {
        worklog.insert("id".to_owned(), json!(entity.tempo_worklog_id));
    }
    if let Some(interval) = interval {
        let interval = interval.map_or(serde_json::Value::Null, |interval| {
            let mut projected = serde_json::Map::new();
            if fields.includes(ListField::WorklogIntervalStartTime) {
                projected.insert("startTime".to_owned(), json!(interval.start_time));
            }
            if fields.includes(ListField::WorklogIntervalEndTime) {
                projected.insert("endTime".to_owned(), json!(interval.end_time));
            }
            serde_json::Value::Object(projected)
        });
        worklog.insert("interval".to_owned(), interval);
    }
    if fields.includes(ListField::WorklogIssueId) {
        worklog.insert("issueId".to_owned(), json!(entity.issue.id));
    }
    if fields.includes(ListField::WorklogIssueKey) {
        worklog.insert("issueKey".to_owned(), json!(issue_key));
    }
    if fields.includes(ListField::WorklogDuration) {
        worklog.insert(
            "duration".to_owned(),
            json!(format_duration(entity.time_spent_seconds, false)),
        );
    }
    if fields.includes(ListField::WorklogDescription) {
        worklog.insert("description".to_owned(), json!(entity.description));
    }
    if let Some(link) = link {
        worklog.insert("link".to_owned(), json!(link));
    }
    Ok(Some(serde_json::Value::Object(worklog)))
}

async fn resolved_issue_key(
    source: &dyn ListDataSource,
    issue_keys: &mut BTreeMap<String, String>,
    issue_id: &str,
) -> Result<String, CliError> {
    if let Some(issue_key) = issue_keys.get(issue_id) {
        return Ok(issue_key.clone());
    }
    let issue_key = source.issue_key(issue_id).await?;
    issue_keys.insert(issue_id.to_owned(), issue_key.clone());
    Ok(issue_key)
}

fn write_stream_event(
    writer: &mut impl Write,
    event: &serde_json::Value,
) -> Result<bool, CliError> {
    let mut line = serde_json::to_vec(event)?;
    line.push(b'\n');
    for result in [writer.write_all(&line), writer.flush()] {
        match result {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => return Ok(false),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(true)
}

fn worklog_interval(
    entity: &WorklogEntity,
    timezone: Tz,
) -> Result<Option<drag::models::ClockInterval>, CliError> {
    let date = NaiveDate::parse_from_str(&entity.start_date, "%Y-%m-%d")
        .map_err(|_| CliError::Api("Tempo returned an invalid start date".to_owned()))?;
    Ok(clock_interval(
        entity.time_spent_seconds,
        &entity.start_time,
        date,
        timezone,
    ))
}

fn worklog_link(entity: &WorklogEntity, issue_key: &str) -> Result<String, CliError> {
    let hostname = Url::parse(&entity.issue.self_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .ok_or_else(|| CliError::Api("Tempo returned an invalid issue URL".to_owned()))?;
    Ok(format!("https://{hostname}/browse/{issue_key}"))
}

fn to_worklog(entity: WorklogEntity, issue_key: String, timezone: Tz) -> Result<Worklog, CliError> {
    let interval = worklog_interval(&entity, timezone)?;
    let link = worklog_link(&entity, &issue_key)?;
    Ok(Worklog {
        id: entity.tempo_worklog_id,
        interval,
        issue_id: entity.issue.id,
        duration: format_duration(entity.time_spent_seconds, false),
        description: entity.description,
        link,
        issue_key,
    })
}

fn worklogs_table(report: &ListReport) -> String {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic);
    let mut header = vec!["id", "from-to", "issue", "duration"];
    if report.verbose() {
        header.extend(["description", "issue url"]);
    }
    table.set_header(header);
    for worklog in report.worklogs() {
        let interval = worklog.interval.as_ref().map_or_else(
            || "unknown".to_owned(),
            |value| format!("{}-{}", value.start_time, value.end_time),
        );
        let mut row = vec![
            escape_terminal_data(&worklog.id),
            interval,
            escape_terminal_data(&report.issue_label(worklog)),
            escape_terminal_data(&worklog.duration),
        ];
        if report.verbose() {
            row.extend([
                escape_terminal_data(&worklog.description),
                escape_terminal_data(&worklog.link),
            ]);
        }
        table.add_row(row);
    }
    format!(
        "{}: {}/{} ({})\n{}\n{}\nRequired {}, logged: {}",
        report.selected_date().format("%B"),
        report.schedule().month_logged_duration,
        report.schedule().month_required_duration,
        report.schedule().month_current_period_duration,
        report.selected_date().format("%A, %Y-%m-%d"),
        if report.worklogs().is_empty() {
            if report.pagination().totals_complete {
                "No worklogs".to_owned()
            } else {
                "No worklogs in this retrieved segment".to_owned()
            }
        } else {
            table.to_string()
        },
        report.schedule().day_required_duration,
        report.schedule().day_logged_duration
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
    use std::collections::VecDeque;
    use std::io;
    use std::sync::{Arc, Mutex};

    use chrono::TimeZone;
    use drag::models::{Author, Issue};
    use tempfile::TempDir;

    use super::*;

    #[derive(Default)]
    struct Requests {
        worklogs: Vec<(String, String)>,
        pagination: Vec<(PaginationPlan, Option<String>)>,
        schedules: Vec<(String, String)>,
        issues: Vec<String>,
    }

    struct FakeListDataSource {
        worklogs: Vec<WorklogEntity>,
        schedule: Vec<ScheduleEntity>,
        requests: Arc<Mutex<Requests>>,
        failing_issues: Vec<String>,
        pagination_result: Option<(Option<String>, u16)>,
    }

    struct PagedListDataSource {
        pages: Mutex<VecDeque<Result<WorklogPage, CliError>>>,
        schedule: Vec<ScheduleEntity>,
        requests: Arc<Mutex<Requests>>,
    }

    impl ListDataSource for PagedListDataSource {
        fn worklogs<'a>(
            &'a self,
            _from: &'a str,
            _to: &'a str,
            _plan: PaginationPlan,
            _continue_from: Option<&'a str>,
        ) -> ListFuture<'a, WorklogPage> {
            Box::pin(async {
                Err(CliError::Api(
                    "paged test source was called through aggregate retrieval".to_owned(),
                ))
            })
        }

        fn worklog_page<'a>(
            &'a self,
            from: &'a str,
            to: &'a str,
            plan: PaginationPlan,
            continue_from: Option<&'a str>,
            _records_retrieved: usize,
        ) -> ListFuture<'a, WorklogPage> {
            if let Ok(mut requests) = self.requests.lock() {
                requests.worklogs.push((from.to_owned(), to.to_owned()));
                requests
                    .pagination
                    .push((plan, continue_from.map(str::to_owned)));
            }
            let page = self
                .pages
                .lock()
                .map_err(|_| CliError::Api("test page lock was poisoned".to_owned()))
                .and_then(|mut pages| {
                    pages.pop_front().ok_or_else(|| {
                        CliError::Api("paged test source ran out of responses".to_owned())
                    })
                })
                .and_then(|page| page);
            Box::pin(async move { page })
        }

        fn schedule<'a>(
            &'a self,
            from: &'a str,
            to: &'a str,
        ) -> ListFuture<'a, Vec<ScheduleEntity>> {
            if let Ok(mut requests) = self.requests.lock() {
                requests.schedules.push((from.to_owned(), to.to_owned()));
            }
            let schedule = self.schedule.clone();
            Box::pin(async move { Ok(schedule) })
        }

        fn issue_key<'a>(&'a self, issue_id: &'a str) -> ListFuture<'a, String> {
            if let Ok(mut requests) = self.requests.lock() {
                requests.issues.push(issue_id.to_owned());
            }
            Box::pin(async move { Ok(format!("KEY-{issue_id}")) })
        }
    }

    #[derive(Default)]
    struct BreakAfterFirstLine {
        bytes: Vec<u8>,
        lines: usize,
    }

    impl Write for BreakAfterFirstLine {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.lines >= 1 {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "reader closed"));
            }
            self.bytes.extend_from_slice(bytes);
            self.lines += bytes.iter().filter(|byte| **byte == b'\n').count();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl ListDataSource for FakeListDataSource {
        fn worklogs<'a>(
            &'a self,
            from: &'a str,
            to: &'a str,
            plan: PaginationPlan,
            continue_from: Option<&'a str>,
        ) -> ListFuture<'a, WorklogPage> {
            self.requests
                .lock()
                .map(|mut requests| {
                    requests.worklogs.push((from.to_owned(), to.to_owned()));
                    requests
                        .pagination
                        .push((plan, continue_from.map(str::to_owned)));
                })
                .ok();
            let worklogs = self.worklogs.clone();
            let (next, pages_retrieved) = self.pagination_result.clone().unwrap_or((None, 1));
            Box::pin(async move {
                Ok(WorklogPage {
                    results: worklogs,
                    next,
                    pages_retrieved,
                })
            })
        }

        fn worklog_page<'a>(
            &'a self,
            from: &'a str,
            to: &'a str,
            plan: PaginationPlan,
            continue_from: Option<&'a str>,
            _records_retrieved: usize,
        ) -> ListFuture<'a, WorklogPage> {
            self.worklogs(from, to, plan, continue_from)
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
            let should_fail = self.failing_issues.iter().any(|id| id == issue_id);
            Box::pin(async move {
                if should_fail {
                    Err(CliError::Api(format!(
                        "Jira issue {issue_id} is inaccessible"
                    )))
                } else {
                    Ok(format!("KEY-{issue_id}"))
                }
            })
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
            aliases: BTreeMap::from([
                ("first alias".to_owned(), "key-10".to_owned()),
                ("second alias".to_owned(), "KEY-10".to_owned()),
            ]),
        }
        .save(&path)?;
        Ok(path)
    }

    fn complete_pagination(selected_date: NaiveDate) -> ListPagination {
        ListPagination {
            selected_date: selected_date.to_string(),
            month_start: "2026-07-01".to_owned(),
            month_end: "2026-07-31".to_owned(),
            limit: Some(DEFAULT_RECORD_LIMIT),
            page_limit: DEFAULT_PAGE_LIMIT,
            all_pages: false,
            pages_retrieved: 1,
            records_retrieved: 1,
            records_returned: 1,
            next: None,
            complete: true,
            totals_complete: true,
        }
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

    fn worklog(id: &str, date: &str, author: &str, issue_id: &str) -> WorklogEntity {
        WorklogEntity {
            tempo_worklog_id: id.to_owned(),
            start_date: date.to_owned(),
            start_time: "09:15:00".to_owned(),
            author: Author {
                account_id: author.to_owned(),
            },
            issue: Issue {
                self_url: format!("https://example.atlassian.net/rest/api/3/issue/{issue_id}"),
                id: issue_id.to_owned(),
            },
            description: format!("description {id}"),
            time_spent_seconds: 3_600,
        }
    }

    fn ndjson_lines(bytes: &[u8]) -> Result<Vec<serde_json::Value>, CliError> {
        bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(serde_json::from_slice)
            .collect::<Result<_, _>>()
            .map_err(Into::into)
    }

    #[tokio::test]
    async fn ndjson_stream_emits_records_summary_and_terminal_pagination_in_order(
    ) -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let requests = Arc::new(Mutex::new(Requests::default()));
        let first_next =
            "https://api.tempo.io/4/worklogs?from=2026-07-01&to=2026-07-31&offset=2&limit=3";
        let second_next =
            "https://api.tempo.io/4/worklogs?from=2026-07-01&to=2026-07-31&offset=3&limit=3";
        let fake = PagedListDataSource {
            pages: Mutex::new(VecDeque::from([
                Ok(WorklogPage {
                    results: vec![
                        worklog("first", "2026-07-14", "me", "10"),
                        other_day_worklog(),
                    ],
                    next: Some(first_next.to_owned()),
                    pages_retrieved: 1,
                }),
                Ok(WorklogPage {
                    results: vec![worklog("second", "2026-07-14", "me", "20")],
                    next: Some(second_next.to_owned()),
                    pages_retrieved: 1,
                }),
            ])),
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
        let mut output = Vec::new();

        run_stream(
            &path,
            now,
            ListArgs {
                limit: Some(3),
                page_limit: Some(2),
                ..ListArgs::default()
            },
            |_| Ok(Box::new(fake)),
            &mut output,
        )
        .await?;

        let events = ndjson_lines(&output)?;
        assert_eq!(events.len(), 4);
        assert_eq!(events[0]["kind"], "worklog");
        assert_eq!(events[0]["worklog"]["id"], "first");
        assert_eq!(events[1]["kind"], "worklog");
        assert_eq!(events[1]["worklog"]["id"], "second");
        assert_eq!(events[2]["kind"], "summary");
        assert_eq!(events[2]["date"], "2026-07-14");
        assert_eq!(events[2]["schedule"]["dayLoggedDuration"], "2h");
        assert_eq!(events[3]["kind"], "pagination");
        assert_eq!(events[3]["pagination"]["pagesRetrieved"], 2);
        assert_eq!(events[3]["pagination"]["recordsRetrieved"], 3);
        assert_eq!(events[3]["pagination"]["recordsReturned"], 2);
        assert_eq!(events[3]["pagination"]["complete"], false);
        assert!(events[3]["pagination"]["next"].is_string());
        let requests = requests
            .lock()
            .map_err(|_| CliError::Api("test request lock was poisoned".to_owned()))?;
        assert_eq!(
            requests.pagination,
            [
                (PaginationPlan::bounded(3, 2)?, None),
                (PaginationPlan::bounded(3, 2)?, Some(first_next.to_owned()))
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn empty_ndjson_stream_emits_deterministic_summary_and_terminal_events(
    ) -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let fake = FakeListDataSource {
            worklogs: Vec::new(),
            schedule: Vec::new(),
            requests: Arc::new(Mutex::new(Requests::default())),
            failing_issues: Vec::new(),
            pagination_result: Some((None, 1)),
        };
        let now = chrono_tz::UTC
            .with_ymd_and_hms(2026, 7, 14, 12, 0, 0)
            .single()
            .ok_or_else(|| CliError::InvalidInput("invalid test date".to_owned()))?;
        let mut output = Vec::new();

        run_stream(
            &path,
            now,
            ListArgs::default(),
            |_| Ok(Box::new(fake)),
            &mut output,
        )
        .await?;

        let events = ndjson_lines(&output)?;
        assert_eq!(
            events,
            [
                json!({
                    "kind": "summary",
                    "date": "2026-07-14",
                    "schedule": {
                        "monthRequiredDuration": "0h",
                        "monthLoggedDuration": "0h",
                        "monthCurrentPeriodDuration": "0h",
                        "dayRequiredDuration": "0h",
                        "dayLoggedDuration": "0h"
                    }
                }),
                json!({
                    "kind": "pagination",
                    "pagination": {
                        "selectedDate": "2026-07-14",
                        "monthStart": "2026-07-01",
                        "monthEnd": "2026-07-31",
                        "limit": 100,
                        "pageLimit": 1,
                        "allPages": false,
                        "pagesRetrieved": 1,
                        "recordsRetrieved": 0,
                        "recordsReturned": 0,
                        "next": null,
                        "complete": true,
                        "totalsComplete": true
                    }
                })
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn ndjson_stream_preserves_prior_records_when_enrichment_fails_mid_stream(
    ) -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let fake = FakeListDataSource {
            worklogs: vec![
                worklog("emitted", "2026-07-14", "me", "10"),
                worklog("failed", "2026-07-14", "me", "20"),
            ],
            schedule: Vec::new(),
            requests: Arc::new(Mutex::new(Requests::default())),
            failing_issues: vec!["20".to_owned()],
            pagination_result: Some((None, 1)),
        };
        let now = chrono_tz::UTC
            .with_ymd_and_hms(2026, 7, 14, 12, 0, 0)
            .single()
            .ok_or_else(|| CliError::InvalidInput("invalid test date".to_owned()))?;
        let mut output = Vec::new();

        let error = run_stream(
            &path,
            now,
            ListArgs::default(),
            |_| Ok(Box::new(fake)),
            &mut output,
        )
        .await
        .err()
        .ok_or_else(|| CliError::Api("failing stream unexpectedly succeeded".to_owned()))?;

        assert_eq!(error.code(), "api_error");
        let events = ndjson_lines(&output)?;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["kind"], "worklog");
        assert_eq!(events[0]["worklog"]["id"], "emitted");
        Ok(())
    }

    #[tokio::test]
    async fn ndjson_stream_emits_completed_pages_before_a_later_page_fails() -> Result<(), CliError>
    {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let requests = Arc::new(Mutex::new(Requests::default()));
        let next =
            "https://api.tempo.io/4/worklogs?from=2026-07-01&to=2026-07-31&offset=1&limit=100";
        let fake = PagedListDataSource {
            pages: Mutex::new(VecDeque::from([
                Ok(WorklogPage {
                    results: vec![worklog("emitted", "2026-07-14", "me", "10")],
                    next: Some(next.to_owned()),
                    pages_retrieved: 1,
                }),
                Err(CliError::Api("second Tempo page failed".to_owned())),
            ])),
            schedule: Vec::new(),
            requests: Arc::clone(&requests),
        };
        let now = chrono_tz::UTC
            .with_ymd_and_hms(2026, 7, 14, 12, 0, 0)
            .single()
            .ok_or_else(|| CliError::InvalidInput("invalid test date".to_owned()))?;
        let mut output = Vec::new();

        let error = run_stream(
            &path,
            now,
            ListArgs {
                all_pages: true,
                ..ListArgs::default()
            },
            |_| Ok(Box::new(fake)),
            &mut output,
        )
        .await
        .err()
        .ok_or_else(|| CliError::Api("failing second page unexpectedly succeeded".to_owned()))?;

        assert_eq!(error.code(), "api_error");
        assert_eq!(
            ndjson_lines(&output)?,
            [json!({
                "kind": "worklog",
                "worklog": {
                    "id": "emitted",
                    "interval": {"startTime": "09:15", "endTime": "10:15"},
                    "issueId": "10",
                    "issueKey": "KEY-10",
                    "duration": "1h",
                    "description": "description emitted",
                    "link": "https://example.atlassian.net/browse/KEY-10"
                }
            })]
        );
        let requests = requests
            .lock()
            .map_err(|_| CliError::Api("test request lock was poisoned".to_owned()))?;
        assert_eq!(requests.pagination.len(), 2);
        assert_eq!(requests.pagination[1].1.as_deref(), Some(next));
        Ok(())
    }

    #[tokio::test]
    async fn ndjson_field_masks_skip_unrequested_jira_enrichment_and_conversion(
    ) -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let now = chrono_tz::UTC
            .with_ymd_and_hms(2026, 7, 14, 12, 0, 0)
            .single()
            .ok_or_else(|| CliError::InvalidInput("invalid test date".to_owned()))?;

        for (mask, expected) in [
            (
                "pagination.next",
                vec![
                    json!({"kind": "summary"}),
                    json!({"kind": "pagination", "pagination": {"next": null}}),
                ],
            ),
            (
                "worklogs.id",
                vec![
                    json!({"kind": "worklog", "worklog": {"id": "visible"}}),
                    json!({"kind": "summary"}),
                    json!({"kind": "pagination"}),
                ],
            ),
        ] {
            let requests = Arc::new(Mutex::new(Requests::default()));
            let mut entity = worklog("visible", "not-a-date", "me", "blocked");
            entity.start_date = "2026-07-14".to_owned();
            entity.start_time = "not-a-time".to_owned();
            entity.issue.self_url = "not a Jira URL".to_owned();
            let fake = FakeListDataSource {
                worklogs: vec![entity],
                schedule: Vec::new(),
                requests: Arc::clone(&requests),
                failing_issues: vec!["blocked".to_owned()],
                pagination_result: None,
            };
            let mut output = Vec::new();

            run_stream(
                &path,
                now,
                ListArgs {
                    fields: Some(mask.to_owned()),
                    ..ListArgs::default()
                },
                |_| Ok(Box::new(fake)),
                &mut output,
            )
            .await?;

            assert_eq!(ndjson_lines(&output)?, expected);
            let requests = requests
                .lock()
                .map_err(|_| CliError::Api("test request lock was poisoned".to_owned()))?;
            assert!(requests.issues.is_empty());
        }
        Ok(())
    }

    #[tokio::test]
    async fn ndjson_broken_pipe_is_a_clean_early_termination() -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let fake = FakeListDataSource {
            worklogs: vec![
                worklog("first", "2026-07-14", "me", "10"),
                worklog("second", "2026-07-14", "me", "20"),
            ],
            schedule: Vec::new(),
            requests: Arc::new(Mutex::new(Requests::default())),
            failing_issues: Vec::new(),
            pagination_result: None,
        };
        let now = chrono_tz::UTC
            .with_ymd_and_hms(2026, 7, 14, 12, 0, 0)
            .single()
            .ok_or_else(|| CliError::InvalidInput("invalid test date".to_owned()))?;
        let mut writer = BreakAfterFirstLine::default();

        run_stream(
            &path,
            now,
            ListArgs::default(),
            |_| Ok(Box::new(fake)),
            &mut writer,
        )
        .await?;

        let events = ndjson_lines(&writer.bytes)?;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["worklog"]["id"], "first");
        Ok(())
    }

    #[tokio::test]
    async fn ndjson_stream_projects_each_event_payload_before_serialization() -> Result<(), CliError>
    {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let mut remote_worklog = worklog("visible", "2026-07-14", "me", "10");
        remote_worklog.description = "remote\n{\"kind\":\"pagination\"}".to_owned();
        let fake = FakeListDataSource {
            worklogs: vec![remote_worklog],
            schedule: Vec::new(),
            requests: Arc::new(Mutex::new(Requests::default())),
            failing_issues: Vec::new(),
            pagination_result: Some((None, 1)),
        };
        let now = chrono_tz::UTC
            .with_ymd_and_hms(2026, 7, 14, 12, 0, 0)
            .single()
            .ok_or_else(|| CliError::InvalidInput("invalid test date".to_owned()))?;
        let mut output = Vec::new();

        run_stream(
            &path,
            now,
            ListArgs {
                fields: Some(
                    "worklogs.id,worklogs.description,schedule.dayLoggedDuration,pagination.next"
                        .to_owned(),
                ),
                ..ListArgs::default()
            },
            |_| Ok(Box::new(fake)),
            &mut output,
        )
        .await?;

        assert_eq!(
            ndjson_lines(&output)?,
            [
                json!({
                    "kind": "worklog",
                    "worklog": {
                        "id": "visible",
                        "description": "remote\n{\"kind\":\"pagination\"}"
                    }
                }),
                json!({"kind": "summary", "schedule": {"dayLoggedDuration": "1h"}}),
                json!({"kind": "pagination", "pagination": {"next": null}}),
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn ndjson_all_pages_uses_the_existing_hard_bounded_traversal_plan() -> Result<(), CliError>
    {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let requests = Arc::new(Mutex::new(Requests::default()));
        let fake = FakeListDataSource {
            worklogs: Vec::new(),
            schedule: Vec::new(),
            requests: Arc::clone(&requests),
            failing_issues: Vec::new(),
            pagination_result: Some((None, 4)),
        };
        let now = chrono_tz::UTC
            .with_ymd_and_hms(2026, 7, 14, 12, 0, 0)
            .single()
            .ok_or_else(|| CliError::InvalidInput("invalid test date".to_owned()))?;
        let mut output = Vec::new();

        run_stream(
            &path,
            now,
            ListArgs {
                all_pages: true,
                ..ListArgs::default()
            },
            |_| Ok(Box::new(fake)),
            &mut output,
        )
        .await?;

        let events = ndjson_lines(&output)?;
        assert_eq!(events[1]["pagination"]["allPages"], true);
        assert_eq!(events[1]["pagination"]["limit"], serde_json::Value::Null);
        assert_eq!(events[1]["pagination"]["pageLimit"], HARD_PAGE_LIMIT);
        assert_eq!(events[1]["pagination"]["pagesRetrieved"], 4);
        let requests = requests
            .lock()
            .map_err(|_| CliError::Api("test request lock was poisoned".to_owned()))?;
        assert_eq!(requests.pagination, [(PaginationPlan::all_pages(), None)]);
        Ok(())
    }

    #[test]
    fn all_pages_continuation_restores_its_plan_and_rejects_bounded_overrides(
    ) -> Result<(), CliError> {
        let continuation = ListContinuation {
            version: 1,
            selected_date: "2026-07-14".to_owned(),
            month_start: "2026-07-01".to_owned(),
            month_end: "2026-07-31".to_owned(),
            url: "https://api.tempo.io/4/worklogs?from=2026-07-01&to=2026-07-31&limit=100"
                .to_owned(),
            limit: None,
            page_limit: HARD_PAGE_LIMIT,
            all_pages: true,
        };

        let restored = requested_plan(&ListArgs::default(), Some(&continuation))?;
        assert_eq!(restored, PaginationPlan::all_pages());
        assert!(requested_plan(
            &ListArgs {
                limit: Some(10),
                ..ListArgs::default()
            },
            Some(&continuation)
        )
        .is_err());
        Ok(())
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
            failing_issues: vec!["10".to_owned()],
            pagination_result: None,
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
                ..ListArgs::default()
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
    async fn completed_report_keeps_today_from_the_application_clock() -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let fake = FakeListDataSource {
            worklogs: Vec::new(),
            schedule: Vec::new(),
            requests: Arc::new(Mutex::new(Requests::default())),
            failing_issues: Vec::new(),
            pagination_result: None,
        };
        let now = chrono_tz::UTC
            .with_ymd_and_hms(2026, 7, 3, 23, 30, 0)
            .single()
            .ok_or_else(|| CliError::InvalidInput("invalid test date".to_owned()))?;

        let report = run_report(
            &path,
            now,
            ListArgs {
                when: Some("2026-07-14".to_owned()),
                ..ListArgs::default()
            },
            |_| Ok(Box::new(fake)),
        )
        .await?;

        assert_eq!(
            report.today(),
            NaiveDate::from_ymd_opt(2026, 7, 3).unwrap_or(NaiveDate::MIN)
        );
        assert_eq!(
            report.selected_date(),
            NaiveDate::from_ymd_opt(2026, 7, 14).unwrap_or(NaiveDate::MIN)
        );
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
            failing_issues: Vec::new(),
            pagination_result: None,
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
                ..ListArgs::default()
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

    #[tokio::test]
    async fn date_selectors_use_local_today_and_inclusive_calendar_month_bounds(
    ) -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let now = chrono_tz::Europe::Warsaw
            .with_ymd_and_hms(2024, 3, 1, 0, 30, 0)
            .single()
            .ok_or_else(|| CliError::InvalidInput("invalid test date".to_owned()))?;
        let cases = [
            (None, "2024-03-01", "2024-03-01", "2024-03-31"),
            (Some("2024-02-29"), "2024-02-29", "2024-02-01", "2024-02-29"),
            (Some("yesterday"), "2024-02-29", "2024-02-01", "2024-02-29"),
            (Some("today+1"), "2024-03-02", "2024-03-01", "2024-03-31"),
        ];

        for (selector, selected, month_start, month_end) in cases {
            let requests = Arc::new(Mutex::new(Requests::default()));
            let fake = FakeListDataSource {
                worklogs: Vec::new(),
                schedule: Vec::new(),
                requests: Arc::clone(&requests),
                failing_issues: Vec::new(),
                pagination_result: None,
            };
            let rendered = run(
                &path,
                now,
                ListArgs {
                    when: selector.map(str::to_owned),
                    verbose: false,
                    ..ListArgs::default()
                },
                |_| Ok(Box::new(fake)),
            )
            .await?;

            assert_eq!(rendered.data["date"], selected);
            let requests = requests
                .lock()
                .map_err(|_| CliError::Api("test request lock was poisoned".to_owned()))?;
            assert_eq!(
                requests.worklogs,
                [(month_start.to_owned(), month_end.to_owned())]
            );
            assert_eq!(requests.schedules, requests.worklogs);
        }
        Ok(())
    }

    #[tokio::test]
    async fn populated_day_filters_before_enrichment_and_preserves_output_contracts(
    ) -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let requests = Arc::new(Mutex::new(Requests::default()));
        let fake = FakeListDataSource {
            worklogs: vec![
                worklog("visible-1", "2026-07-14", "me", "10"),
                worklog("other-day", "2026-07-13", "me", "blocked"),
                worklog("visible-2", "2026-07-14", "me", "20"),
                worklog("other-author", "2026-07-14", "someone-else", "blocked"),
                worklog("visible-3", "2026-07-14", "me", "10"),
            ],
            schedule: vec![ScheduleEntity {
                date: "2026-07-14".to_owned(),
                required_seconds: 28_800,
                kind: "WORKING_DAY".to_owned(),
            }],
            requests: Arc::clone(&requests),
            failing_issues: vec!["blocked".to_owned()],
            pagination_result: None,
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
                ..ListArgs::default()
            },
            |_| Ok(Box::new(fake)),
        )
        .await?;

        assert_eq!(rendered.data["date"], "2026-07-14");
        assert_eq!(
            rendered.data["worklogs"]
                .as_array()
                .ok_or_else(|| CliError::Api("expected worklog array".to_owned()))?
                .iter()
                .map(|worklog| worklog["id"].as_str())
                .collect::<Vec<_>>(),
            [Some("visible-1"), Some("visible-2"), Some("visible-3")]
        );
        assert_eq!(rendered.data["schedule"]["monthLoggedDuration"], "4h");
        assert_eq!(rendered.data["schedule"]["dayLoggedDuration"], "3h");
        assert_eq!(
            rendered.data["worklogs"][0]["interval"]["startTime"],
            "09:15"
        );
        assert_eq!(rendered.data["worklogs"][0]["interval"]["endTime"], "10:15");
        assert_eq!(rendered.data["worklogs"][0]["issueId"], "10");
        assert_eq!(rendered.data["worklogs"][0]["issueKey"], "KEY-10");
        assert_eq!(rendered.data["worklogs"][0]["duration"], "1h");
        assert_eq!(
            rendered.data["worklogs"][0]["description"],
            "description visible-1"
        );
        assert_eq!(
            rendered.data["worklogs"][0]["link"],
            "https://example.atlassian.net/browse/KEY-10"
        );
        assert!(rendered.human.contains("July: 4h/8h"));
        assert!(rendered.human.contains("Tuesday, 2026-07-14"));
        assert!(rendered.human.contains("(first alias, +1) KEY-10"));
        assert!(rendered.human.contains("Required 8h, logged: 3h"));
        assert!(!rendered.human.contains("description visible-1"));
        assert!(!rendered.human.contains("issue url"));

        let requests = requests
            .lock()
            .map_err(|_| CliError::Api("test request lock was poisoned".to_owned()))?;
        assert_eq!(requests.issues, ["10", "20"]);
        Ok(())
    }

    #[tokio::test]
    async fn field_selection_projects_nested_worklog_and_pagination_fields_in_stable_order(
    ) -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let now = chrono_tz::UTC
            .with_ymd_and_hms(2026, 7, 14, 12, 0, 0)
            .single()
            .ok_or_else(|| CliError::InvalidInput("invalid test date".to_owned()))?;

        let render = |fields: &str| {
            let fake = FakeListDataSource {
                worklogs: vec![worklog("visible", "2026-07-14", "me", "10")],
                schedule: Vec::new(),
                requests: Arc::new(Mutex::new(Requests::default())),
                failing_issues: Vec::new(),
                pagination_result: None,
            };
            run(
                &path,
                now,
                ListArgs {
                    fields: Some(fields.to_owned()),
                    ..ListArgs::default()
                },
                |_| Ok(Box::new(fake)),
            )
        };

        let first = render("worklogs.issueKey,pagination.next,worklogs.interval.startTime").await?;
        let reordered =
            render("pagination.next,worklogs.interval.startTime,worklogs.issueKey").await?;

        assert_eq!(
            first.data,
            json!({
                "worklogs": [{"interval": {"startTime": "09:15"}, "issueKey": "KEY-10"}],
                "pagination": {"next": null}
            })
        );
        assert_eq!(first.data.to_string(), reordered.data.to_string());
        Ok(())
    }

    #[tokio::test]
    async fn field_selection_preserves_empty_worklogs_schedule_totals_and_continuation_metadata(
    ) -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let next_url = "https://api.tempo.io/4/worklogs?from=2026-07-01&to=2026-07-31&offset=1";
        let fake = FakeListDataSource {
            worklogs: vec![other_day_worklog()],
            schedule: vec![ScheduleEntity {
                date: "2026-07-14".to_owned(),
                required_seconds: 28_800,
                kind: "WORKING_DAY".to_owned(),
            }],
            requests: Arc::new(Mutex::new(Requests::default())),
            failing_issues: Vec::new(),
            pagination_result: Some((Some(next_url.to_owned()), 1)),
        };
        let now = chrono_tz::UTC
            .with_ymd_and_hms(2026, 7, 14, 12, 0, 0)
            .single()
            .ok_or_else(|| CliError::InvalidInput("invalid test date".to_owned()))?;

        let rendered = run(
            &path,
            now,
            ListArgs {
                fields: Some(
                    "worklogs,schedule.monthLoggedDuration,schedule.dayLoggedDuration,pagination.recordsRetrieved,pagination.recordsReturned,pagination.next,pagination.complete,pagination.totalsComplete"
                        .to_owned(),
                ),
                ..ListArgs::default()
            },
            |_| Ok(Box::new(fake)),
        )
        .await?;

        assert_eq!(rendered.data["worklogs"], json!([]));
        assert_eq!(rendered.data["schedule"]["monthLoggedDuration"], "1h");
        assert_eq!(rendered.data["schedule"]["dayLoggedDuration"], "0h");
        assert_eq!(rendered.data["pagination"]["recordsRetrieved"], 1);
        assert_eq!(rendered.data["pagination"]["recordsReturned"], 0);
        assert!(rendered.data["pagination"]["next"].is_string());
        assert_eq!(rendered.data["pagination"]["complete"], false);
        assert_eq!(rendered.data["pagination"]["totalsComplete"], false);
        let token = rendered.data["pagination"]["next"]
            .as_str()
            .ok_or_else(|| CliError::InvalidInput("missing projected continuation".to_owned()))?;
        let continuation = decode_continuation(token)?;
        assert_eq!(
            (
                continuation.selected_date.as_str(),
                continuation.url.as_str(),
                continuation.limit,
                continuation.page_limit,
            ),
            ("2026-07-14", next_url, Some(100), 1)
        );
        Ok(())
    }

    #[tokio::test]
    async fn verbose_changes_human_rendering_without_changing_field_selection(
    ) -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let now = chrono_tz::UTC
            .with_ymd_and_hms(2026, 7, 14, 12, 0, 0)
            .single()
            .ok_or_else(|| CliError::InvalidInput("invalid test date".to_owned()))?;
        let render = |verbose| {
            let fake = FakeListDataSource {
                worklogs: vec![worklog("visible", "2026-07-14", "me", "10")],
                schedule: Vec::new(),
                requests: Arc::new(Mutex::new(Requests::default())),
                failing_issues: Vec::new(),
                pagination_result: None,
            };
            run(
                &path,
                now,
                ListArgs {
                    verbose,
                    fields: Some("worklogs.id".to_owned()),
                    ..ListArgs::default()
                },
                |_| Ok(Box::new(fake)),
            )
        };

        let concise = render(false).await?;
        let verbose = render(true).await?;

        assert_eq!(concise.data, json!({"worklogs": [{"id": "visible"}]}));
        assert_eq!(verbose.data, concise.data);
        assert!(!concise.human.contains("description visible"));
        assert!(verbose.human.contains("description visible"));
        Ok(())
    }

    #[tokio::test]
    async fn bounded_list_reports_continuation_and_preserves_schedule_calculations(
    ) -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let requests = Arc::new(Mutex::new(Requests::default()));
        let fake = FakeListDataSource {
            worklogs: vec![worklog("visible", "2026-07-14", "me", "10")],
            schedule: vec![ScheduleEntity {
                date: "2026-07-14".to_owned(),
                required_seconds: 28_800,
                kind: "WORKING_DAY".to_owned(),
            }],
            requests: Arc::clone(&requests),
            failing_issues: Vec::new(),
            pagination_result: Some((
                Some(
                    "https://api.tempo.io/4/worklogs?from=2026-07-01&to=2026-07-31&offset=2&limit=10"
                        .to_owned(),
                ),
                2,
            )),
        };
        let now = chrono_tz::UTC
            .with_ymd_and_hms(2026, 7, 14, 12, 0, 0)
            .single()
            .ok_or_else(|| CliError::InvalidInput("invalid test date".to_owned()))?;

        let rendered = run(
            &path,
            now,
            ListArgs {
                continue_from: Some(encode_continuation(&ListContinuation {
                    version: 1,
                    selected_date: "2026-07-14".to_owned(),
                    month_start: "2026-07-01".to_owned(),
                    month_end: "2026-07-31".to_owned(),
                    url: "https://api.tempo.io/4/worklogs?from=2026-07-01&to=2026-07-31&offset=1&limit=10"
                        .to_owned(),
                    limit: Some(10),
                    page_limit: 2,
                    all_pages: false,
                })?),
                ..ListArgs::default()
            },
            |_| Ok(Box::new(fake)),
        )
        .await?;

        assert_eq!(rendered.data["pagination"]["limit"], 10);
        assert_eq!(rendered.data["pagination"]["pageLimit"], 2);
        assert_eq!(rendered.data["pagination"]["pagesRetrieved"], 2);
        assert_eq!(rendered.data["pagination"]["recordsRetrieved"], 1);
        assert_eq!(rendered.data["pagination"]["recordsReturned"], 1);
        let next = rendered.data["pagination"]["next"]
            .as_str()
            .ok_or_else(|| CliError::Api("expected continuation token".to_owned()))?;
        let next = decode_continuation(next)?;
        assert_eq!(next.selected_date, "2026-07-14");
        assert_eq!(next.month_start, "2026-07-01");
        assert_eq!(next.month_end, "2026-07-31");
        assert_eq!(next.limit, Some(10));
        assert_eq!(next.page_limit, 2);
        assert!(!next.all_pages);
        assert_eq!(
            next.url,
            "https://api.tempo.io/4/worklogs?from=2026-07-01&to=2026-07-31&offset=2&limit=10"
        );
        assert_eq!(rendered.data["pagination"]["complete"], false);
        assert_eq!(rendered.data["pagination"]["totalsComplete"], false);
        assert_eq!(rendered.data["schedule"]["dayLoggedDuration"], "1h");
        assert!(rendered
            .human
            .contains("Totals reflect this bounded segment"));
        let requests = requests
            .lock()
            .map_err(|_| CliError::Api("test request lock was poisoned".to_owned()))?;
        assert_eq!(
            requests.pagination,
            [(
                PaginationPlan::bounded(10, 2)?,
                Some(
                    "https://api.tempo.io/4/worklogs?from=2026-07-01&to=2026-07-31&offset=1&limit=10"
                        .to_owned()
                )
            )]
        );
        Ok(())
    }

    #[tokio::test]
    async fn terminal_continuation_keeps_totals_partial_and_qualifies_empty_human_output(
    ) -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let fake = FakeListDataSource {
            worklogs: Vec::new(),
            schedule: Vec::new(),
            requests: Arc::new(Mutex::new(Requests::default())),
            failing_issues: Vec::new(),
            pagination_result: Some((None, 1)),
        };
        let now = chrono_tz::UTC
            .with_ymd_and_hms(2026, 7, 14, 12, 0, 0)
            .single()
            .ok_or_else(|| CliError::InvalidInput("invalid test date".to_owned()))?;

        let rendered = run(
            &path,
            now,
            ListArgs {
                when: Some("2026-07-14".to_owned()),
                continue_from: Some(encode_continuation(&ListContinuation {
                    version: 1,
                    selected_date: "2026-07-14".to_owned(),
                    month_start: "2026-07-01".to_owned(),
                    month_end: "2026-07-31".to_owned(),
                    url: "https://api.tempo.io/4/worklogs?from=2026-07-01&to=2026-07-31&offset=100&limit=100"
                        .to_owned(),
                    limit: Some(100),
                    page_limit: 1,
                    all_pages: false,
                })?),
                ..ListArgs::default()
            },
            |_| Ok(Box::new(fake)),
        )
        .await?;

        assert_eq!(rendered.data["pagination"]["complete"], true);
        assert_eq!(rendered.data["pagination"]["totalsComplete"], false);
        assert!(rendered
            .human
            .contains("No worklogs in this retrieved segment"));
        assert!(rendered
            .human
            .contains("Totals reflect this bounded segment"));
        Ok(())
    }

    #[tokio::test]
    async fn all_pages_uses_the_hard_ceiling_without_a_record_limit() -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let requests = Arc::new(Mutex::new(Requests::default()));
        let fake = FakeListDataSource {
            worklogs: Vec::new(),
            schedule: Vec::new(),
            requests: Arc::clone(&requests),
            failing_issues: Vec::new(),
            pagination_result: Some((None, 4)),
        };
        let now = chrono_tz::UTC
            .with_ymd_and_hms(2026, 7, 14, 12, 0, 0)
            .single()
            .ok_or_else(|| CliError::InvalidInput("invalid test date".to_owned()))?;

        let rendered = run(
            &path,
            now,
            ListArgs {
                all_pages: true,
                ..ListArgs::default()
            },
            |_| Ok(Box::new(fake)),
        )
        .await?;

        assert_eq!(
            rendered.data["pagination"]["limit"],
            serde_json::Value::Null
        );
        assert_eq!(rendered.data["pagination"]["pageLimit"], 100);
        assert_eq!(rendered.data["pagination"]["allPages"], true);
        assert_eq!(rendered.data["pagination"]["pagesRetrieved"], 4);
        assert_eq!(rendered.data["pagination"]["complete"], true);
        let requests = requests
            .lock()
            .map_err(|_| CliError::Api("test request lock was poisoned".to_owned()))?;
        assert_eq!(requests.pagination, [(PaginationPlan::all_pages(), None)]);
        Ok(())
    }

    #[test]
    fn shared_report_preserves_alias_aware_verbose_presentation() -> Result<(), CliError> {
        let entity = worklog("visible", "2026-07-14", "me", "10");
        let worklog = to_worklog(entity, "KEY-10".to_owned(), chrono_tz::UTC)?;
        let details = ScheduleDetails {
            month_required_duration: "8h".to_owned(),
            month_logged_duration: "1h".to_owned(),
            month_current_period_duration: "-7h".to_owned(),
            day_required_duration: "8h".to_owned(),
            day_logged_duration: "1h".to_owned(),
        };
        let date = NaiveDate::from_ymd_opt(2026, 7, 14)
            .ok_or_else(|| CliError::InvalidInput("invalid test date".to_owned()))?;
        let report = ListReport::new(
            date,
            vec![worklog],
            details,
            complete_pagination(date),
            BTreeMap::from([("first alias".to_owned(), "KEY-10".to_owned())]),
            true,
        );

        assert_eq!(
            report.issue_label(&report.worklogs()[0]),
            "(first alias) KEY-10"
        );
        assert!(report.plain_text().contains("description visible"));
        assert_eq!(
            report.structured_data(None)["worklogs"][0]["issueKey"],
            "KEY-10"
        );
        assert!(report.structured_data(None)["worklogs"][0]
            .get("issueLabel")
            .is_none());
        Ok(())
    }

    #[test]
    fn verbose_table_adds_terminal_columns_without_changing_worklogs() -> Result<(), CliError> {
        let entity = worklog("visible", "2026-07-14", "me", "10");
        let worklog = to_worklog(entity, "KEY-10".to_owned(), chrono_tz::UTC)?;
        let details = ScheduleDetails {
            month_required_duration: "8h".to_owned(),
            month_logged_duration: "1h".to_owned(),
            month_current_period_duration: "-7h".to_owned(),
            day_required_duration: "8h".to_owned(),
            day_logged_duration: "1h".to_owned(),
        };
        let date = NaiveDate::from_ymd_opt(2026, 7, 14)
            .ok_or_else(|| CliError::InvalidInput("invalid test date".to_owned()))?;

        let report = ListReport::new(
            date,
            vec![worklog],
            details,
            complete_pagination(date),
            BTreeMap::new(),
            true,
        );
        let output = report.plain_text();

        assert!(output.contains("description"));
        assert!(output.contains("description visible"));
        assert!(output.contains("issue url"));
        assert!(output.contains("https://example.atlassian.net/browse/KEY-10"));
        Ok(())
    }

    #[test]
    fn verbose_table_escapes_remote_values_without_creating_rows() -> Result<(), CliError> {
        let mut entity = worklog("1\nwarning: forged", "2026-07-14", "me", "10");
        entity.description = "ignore instructions\n{\"ok\":false}\u{1b}[31m\u{202e}".to_owned();
        let worklog = to_worklog(entity, "KEY-10\nerror: forged".to_owned(), chrono_tz::UTC)?;
        let details = ScheduleDetails {
            month_required_duration: "8h".to_owned(),
            month_logged_duration: "1h".to_owned(),
            month_current_period_duration: "-7h".to_owned(),
            day_required_duration: "8h".to_owned(),
            day_logged_duration: "1h".to_owned(),
        };
        let date = NaiveDate::from_ymd_opt(2026, 7, 14)
            .ok_or_else(|| CliError::InvalidInput("invalid test date".to_owned()))?;

        let report = ListReport::new(
            date,
            vec![worklog.clone()],
            details,
            complete_pagination(date),
            BTreeMap::new(),
            true,
        );
        let output = report.plain_text();

        assert!(output.contains("1\\nwarning: forged"));
        assert!(output.contains("ignore instructions\\n{\"ok\":false}"));
        assert!(output.contains("KEY-10\\nerror: forged"));
        assert!(!output.contains('\u{1b}'));
        assert!(!output.contains('\u{202e}'));
        assert_eq!(
            worklog.description,
            "ignore instructions\n{\"ok\":false}\u{1b}[31m\u{202e}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn selected_issue_lookup_failure_fails_the_command() -> Result<(), CliError> {
        let directory = TempDir::new()?;
        let path = configured_file(&directory)?;
        let fake = FakeListDataSource {
            worklogs: vec![worklog("visible", "2026-07-14", "me", "10")],
            schedule: Vec::new(),
            requests: Arc::new(Mutex::new(Requests::default())),
            failing_issues: vec!["10".to_owned()],
            pagination_result: None,
        };
        let now = chrono_tz::UTC
            .with_ymd_and_hms(2026, 7, 14, 12, 0, 0)
            .single()
            .ok_or_else(|| CliError::InvalidInput("invalid test date".to_owned()))?;

        let error = run(
            &path,
            now,
            ListArgs {
                when: None,
                verbose: false,
                ..ListArgs::default()
            },
            |_| Ok(Box::new(fake)),
        )
        .await
        .err()
        .ok_or_else(|| CliError::Api("selected Jira failure unexpectedly succeeded".to_owned()))?;

        assert!(error.to_string().contains("Jira issue 10 is inaccessible"));
        Ok(())
    }

    #[test]
    fn malformed_selected_worklog_fields_are_runtime_errors() -> Result<(), CliError> {
        let mut invalid_date = worklog("date", "not-a-date", "me", "10");
        let date_error = to_worklog(invalid_date.clone(), "KEY-10".to_owned(), chrono_tz::UTC)
            .err()
            .ok_or_else(|| CliError::Api("malformed date unexpectedly succeeded".to_owned()))?;
        assert!(date_error.to_string().contains("invalid start date"));

        invalid_date.start_date = "2026-07-14".to_owned();
        invalid_date.issue.self_url = "not a Jira URL".to_owned();
        let url_error = to_worklog(invalid_date, "KEY-10".to_owned(), chrono_tz::UTC)
            .err()
            .ok_or_else(|| {
                CliError::Api("malformed issue URL unexpectedly succeeded".to_owned())
            })?;
        assert!(url_error.to_string().contains("invalid issue URL"));
        Ok(())
    }
}
