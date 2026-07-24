//! Validated field selection for structured list results.

use std::collections::BTreeSet;

use chrono::NaiveDate;
use serde_json::json;
use thiserror::Error;

use crate::models::{ListPagination, Worklog};
use crate::schedule::ScheduleDetails;

/// Selectable paths in a structured `list` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ListField {
    Date,
    Worklogs,
    WorklogId,
    WorklogInterval,
    WorklogIntervalStartTime,
    WorklogIntervalEndTime,
    WorklogIssueId,
    WorklogIssueKey,
    WorklogDuration,
    WorklogDescription,
    WorklogAttributes,
    WorklogLink,
    Schedule,
    ScheduleMonthRequiredDuration,
    ScheduleMonthLoggedDuration,
    ScheduleMonthCurrentPeriodDuration,
    ScheduleDayRequiredDuration,
    ScheduleDayLoggedDuration,
    Pagination,
    PaginationSelectedDate,
    PaginationMonthStart,
    PaginationMonthEnd,
    PaginationLimit,
    PaginationPageLimit,
    PaginationAllPages,
    PaginationPagesRetrieved,
    PaginationRecordsRetrieved,
    PaginationRecordsReturned,
    PaginationNext,
    PaginationComplete,
    PaginationTotalsComplete,
}

impl ListField {
    const FIELDS: &'static [(Self, &'static str)] = &[
        (Self::Date, "date"),
        (Self::Worklogs, "worklogs"),
        (Self::WorklogId, "worklogs.id"),
        (Self::WorklogInterval, "worklogs.interval"),
        (
            Self::WorklogIntervalStartTime,
            "worklogs.interval.startTime",
        ),
        (Self::WorklogIntervalEndTime, "worklogs.interval.endTime"),
        (Self::WorklogIssueId, "worklogs.issueId"),
        (Self::WorklogIssueKey, "worklogs.issueKey"),
        (Self::WorklogDuration, "worklogs.duration"),
        (Self::WorklogDescription, "worklogs.description"),
        (Self::WorklogAttributes, "worklogs.attributes"),
        (Self::WorklogLink, "worklogs.link"),
        (Self::Schedule, "schedule"),
        (
            Self::ScheduleMonthRequiredDuration,
            "schedule.monthRequiredDuration",
        ),
        (
            Self::ScheduleMonthLoggedDuration,
            "schedule.monthLoggedDuration",
        ),
        (
            Self::ScheduleMonthCurrentPeriodDuration,
            "schedule.monthCurrentPeriodDuration",
        ),
        (
            Self::ScheduleDayRequiredDuration,
            "schedule.dayRequiredDuration",
        ),
        (
            Self::ScheduleDayLoggedDuration,
            "schedule.dayLoggedDuration",
        ),
        (Self::Pagination, "pagination"),
        (Self::PaginationSelectedDate, "pagination.selectedDate"),
        (Self::PaginationMonthStart, "pagination.monthStart"),
        (Self::PaginationMonthEnd, "pagination.monthEnd"),
        (Self::PaginationLimit, "pagination.limit"),
        (Self::PaginationPageLimit, "pagination.pageLimit"),
        (Self::PaginationAllPages, "pagination.allPages"),
        (Self::PaginationPagesRetrieved, "pagination.pagesRetrieved"),
        (
            Self::PaginationRecordsRetrieved,
            "pagination.recordsRetrieved",
        ),
        (
            Self::PaginationRecordsReturned,
            "pagination.recordsReturned",
        ),
        (Self::PaginationNext, "pagination.next"),
        (Self::PaginationComplete, "pagination.complete"),
        (Self::PaginationTotalsComplete, "pagination.totalsComplete"),
    ];

    /// Every accepted path in canonical result order.
    pub fn paths() -> impl ExactSizeIterator<Item = &'static str> {
        Self::FIELDS.iter().map(|(_, path)| *path)
    }

    fn from_path(path: &str) -> Option<Self> {
        Self::FIELDS
            .iter()
            .find_map(|(field, candidate)| (*candidate == path).then_some(*field))
    }

    const fn parent(self) -> Option<Self> {
        match self {
            Self::WorklogId
            | Self::WorklogInterval
            | Self::WorklogIssueId
            | Self::WorklogIssueKey
            | Self::WorklogDuration
            | Self::WorklogDescription
            | Self::WorklogAttributes
            | Self::WorklogLink => Some(Self::Worklogs),
            Self::WorklogIntervalStartTime | Self::WorklogIntervalEndTime => {
                Some(Self::WorklogInterval)
            }
            Self::ScheduleMonthRequiredDuration
            | Self::ScheduleMonthLoggedDuration
            | Self::ScheduleMonthCurrentPeriodDuration
            | Self::ScheduleDayRequiredDuration
            | Self::ScheduleDayLoggedDuration => Some(Self::Schedule),
            Self::PaginationSelectedDate
            | Self::PaginationMonthStart
            | Self::PaginationMonthEnd
            | Self::PaginationLimit
            | Self::PaginationPageLimit
            | Self::PaginationAllPages
            | Self::PaginationPagesRetrieved
            | Self::PaginationRecordsRetrieved
            | Self::PaginationRecordsReturned
            | Self::PaginationNext
            | Self::PaginationComplete
            | Self::PaginationTotalsComplete => Some(Self::Pagination),
            Self::Date | Self::Worklogs | Self::Schedule | Self::Pagination => None,
        }
    }
}

/// A non-empty, duplicate-free list field mask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListFieldMask {
    fields: BTreeSet<ListField>,
}

impl ListFieldMask {
    /// Parses a comma-delimited field mask.
    pub fn parse(value: &str) -> Result<Self, FieldSelectionError> {
        if value.is_empty() {
            return Err(FieldSelectionError::Empty);
        }
        let mut fields = BTreeSet::new();
        for path in value.split(',') {
            if path.is_empty() || path.trim() != path {
                return Err(FieldSelectionError::StructurallyInvalid(path.to_owned()));
            }
            let field = ListField::from_path(path)
                .ok_or_else(|| FieldSelectionError::Unknown(path.to_owned()))?;
            if !fields.insert(field) {
                return Err(FieldSelectionError::Duplicate(path.to_owned()));
            }
        }
        for field in &fields {
            let mut parent = field.parent();
            while let Some(candidate) = parent {
                if fields.contains(&candidate) {
                    return Err(FieldSelectionError::Overlapping);
                }
                parent = candidate.parent();
            }
        }
        Ok(Self { fields })
    }

    /// Whether the mask includes a field directly or through a selected parent.
    #[must_use]
    pub fn includes(&self, field: ListField) -> bool {
        let mut current = Some(field);
        while let Some(candidate) = current {
            if self.fields.contains(&candidate) {
                return true;
            }
            current = candidate.parent();
        }
        false
    }

    /// Whether the mask selects any worklog field.
    #[must_use]
    pub fn selects_worklogs(&self) -> bool {
        [
            ListField::Worklogs,
            ListField::WorklogId,
            ListField::WorklogInterval,
            ListField::WorklogIntervalStartTime,
            ListField::WorklogIntervalEndTime,
            ListField::WorklogIssueId,
            ListField::WorklogIssueKey,
            ListField::WorklogDuration,
            ListField::WorklogDescription,
            ListField::WorklogAttributes,
            ListField::WorklogLink,
        ]
        .into_iter()
        .any(|field| self.includes(field))
    }
}

/// Canonical output and enrichment requirements derived from a list field mask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListProjectionPlan {
    mask: Option<ListFieldMask>,
}

impl ListProjectionPlan {
    /// Selects the complete, unmasked list result.
    #[must_use]
    pub const fn complete() -> Self {
        Self { mask: None }
    }

    /// Builds a selective projection from a validated mask.
    #[must_use]
    pub const fn selected(mask: ListFieldMask) -> Self {
        Self { mask: Some(mask) }
    }

    /// Whether a canonical result field is selected.
    #[must_use]
    pub fn includes(&self, field: ListField) -> bool {
        self.mask.as_ref().is_none_or(|mask| mask.includes(field))
    }

    /// Whether any worklog event or array entry should be emitted.
    #[must_use]
    pub fn selects_worklogs(&self) -> bool {
        self.mask
            .as_ref()
            .is_none_or(ListFieldMask::selects_worklogs)
    }

    /// Whether Jira issue-key resolution is needed by selected worklog leaves.
    #[must_use]
    pub fn requires_issue_key(&self) -> bool {
        self.includes(ListField::WorklogIssueKey) || self.includes(ListField::WorklogLink)
    }

    /// Whether a Jira browse URL must be constructed.
    #[must_use]
    pub fn requires_link(&self) -> bool {
        self.includes(ListField::WorklogLink)
    }

    /// Whether clock interval parsing and calculation is needed.
    #[must_use]
    pub fn requires_interval(&self) -> bool {
        self.includes(ListField::WorklogIntervalStartTime)
            || self.includes(ListField::WorklogIntervalEndTime)
    }

    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.mask.is_none()
    }
}

/// Invalid structured result field selection.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FieldSelectionError {
    #[error("list field mask must not be empty")]
    Empty,
    #[error("unknown list field '{0}'")]
    Unknown(String),
    #[error("duplicate list field '{0}'")]
    Duplicate(String),
    #[error("structurally invalid list field '{0}'")]
    StructurallyInvalid(String),
    #[error("list field mask must not select both a field and one of its descendants")]
    Overlapping,
}

/// Projects a complete list report into the stable shape selected by `plan`.
#[must_use]
pub fn project_list_result(
    date: NaiveDate,
    worklogs: &[Worklog],
    details: &ScheduleDetails,
    pagination: &ListPagination,
    plan: &ListProjectionPlan,
) -> serde_json::Value {
    if plan.is_complete() {
        return json!({
            "date": date,
            "worklogs": worklogs,
            "schedule": details,
            "pagination": pagination,
        });
    }
    let mut result = serde_json::Map::new();
    if plan.includes(ListField::Date) {
        result.insert("date".to_owned(), json!(date));
    }
    if plan.selects_worklogs() {
        result.insert(
            "worklogs".to_owned(),
            serde_json::Value::Array(
                worklogs
                    .iter()
                    .map(|worklog| project_worklog(worklog, plan))
                    .collect(),
            ),
        );
    }
    if selects_schedule(plan) {
        result.insert("schedule".to_owned(), project_schedule(details, plan));
    }
    if selects_pagination(plan) {
        result.insert(
            "pagination".to_owned(),
            project_pagination(pagination, plan),
        );
    }
    serde_json::Value::Object(result)
}

fn project_worklog(worklog: &Worklog, plan: &ListProjectionPlan) -> serde_json::Value {
    let mut result = serde_json::Map::new();
    insert_selected(&mut result, plan, ListField::WorklogId, "id", &worklog.id);
    if plan.includes(ListField::WorklogInterval)
        || plan.includes(ListField::WorklogIntervalStartTime)
        || plan.includes(ListField::WorklogIntervalEndTime)
    {
        let interval = worklog
            .interval
            .as_ref()
            .map_or(serde_json::Value::Null, |interval| {
                let mut projected = serde_json::Map::new();
                insert_selected(
                    &mut projected,
                    plan,
                    ListField::WorklogIntervalStartTime,
                    "startTime",
                    &interval.start_time,
                );
                insert_selected(
                    &mut projected,
                    plan,
                    ListField::WorklogIntervalEndTime,
                    "endTime",
                    &interval.end_time,
                );
                serde_json::Value::Object(projected)
            });
        result.insert("interval".to_owned(), interval);
    }
    insert_selected(
        &mut result,
        plan,
        ListField::WorklogIssueId,
        "issueId",
        &worklog.issue_id,
    );
    insert_selected(
        &mut result,
        plan,
        ListField::WorklogIssueKey,
        "issueKey",
        &worklog.issue_key,
    );
    insert_selected(
        &mut result,
        plan,
        ListField::WorklogDuration,
        "duration",
        &worklog.duration,
    );
    insert_selected(
        &mut result,
        plan,
        ListField::WorklogDescription,
        "description",
        &worklog.description,
    );
    insert_selected(
        &mut result,
        plan,
        ListField::WorklogAttributes,
        "attributes",
        &worklog.attributes,
    );
    insert_selected(
        &mut result,
        plan,
        ListField::WorklogLink,
        "link",
        &worklog.link,
    );
    serde_json::Value::Object(result)
}

fn selects_schedule(plan: &ListProjectionPlan) -> bool {
    [
        ListField::Schedule,
        ListField::ScheduleMonthRequiredDuration,
        ListField::ScheduleMonthLoggedDuration,
        ListField::ScheduleMonthCurrentPeriodDuration,
        ListField::ScheduleDayRequiredDuration,
        ListField::ScheduleDayLoggedDuration,
    ]
    .into_iter()
    .any(|field| plan.includes(field))
}

fn project_schedule(details: &ScheduleDetails, plan: &ListProjectionPlan) -> serde_json::Value {
    let mut result = serde_json::Map::new();
    for (field, name, value) in [
        (
            ListField::ScheduleMonthRequiredDuration,
            "monthRequiredDuration",
            &details.month_required_duration,
        ),
        (
            ListField::ScheduleMonthLoggedDuration,
            "monthLoggedDuration",
            &details.month_logged_duration,
        ),
        (
            ListField::ScheduleMonthCurrentPeriodDuration,
            "monthCurrentPeriodDuration",
            &details.month_current_period_duration,
        ),
        (
            ListField::ScheduleDayRequiredDuration,
            "dayRequiredDuration",
            &details.day_required_duration,
        ),
        (
            ListField::ScheduleDayLoggedDuration,
            "dayLoggedDuration",
            &details.day_logged_duration,
        ),
    ] {
        insert_selected(&mut result, plan, field, name, value);
    }
    serde_json::Value::Object(result)
}

fn selects_pagination(plan: &ListProjectionPlan) -> bool {
    [
        ListField::Pagination,
        ListField::PaginationSelectedDate,
        ListField::PaginationMonthStart,
        ListField::PaginationMonthEnd,
        ListField::PaginationLimit,
        ListField::PaginationPageLimit,
        ListField::PaginationAllPages,
        ListField::PaginationPagesRetrieved,
        ListField::PaginationRecordsRetrieved,
        ListField::PaginationRecordsReturned,
        ListField::PaginationNext,
        ListField::PaginationComplete,
        ListField::PaginationTotalsComplete,
    ]
    .into_iter()
    .any(|field| plan.includes(field))
}

fn project_pagination(pagination: &ListPagination, plan: &ListProjectionPlan) -> serde_json::Value {
    let mut result = serde_json::Map::new();
    insert_selected(
        &mut result,
        plan,
        ListField::PaginationSelectedDate,
        "selectedDate",
        &pagination.selected_date,
    );
    insert_selected(
        &mut result,
        plan,
        ListField::PaginationMonthStart,
        "monthStart",
        &pagination.month_start,
    );
    insert_selected(
        &mut result,
        plan,
        ListField::PaginationMonthEnd,
        "monthEnd",
        &pagination.month_end,
    );
    insert_selected(
        &mut result,
        plan,
        ListField::PaginationLimit,
        "limit",
        pagination.limit,
    );
    insert_selected(
        &mut result,
        plan,
        ListField::PaginationPageLimit,
        "pageLimit",
        pagination.page_limit,
    );
    insert_selected(
        &mut result,
        plan,
        ListField::PaginationAllPages,
        "allPages",
        pagination.all_pages,
    );
    insert_selected(
        &mut result,
        plan,
        ListField::PaginationPagesRetrieved,
        "pagesRetrieved",
        pagination.pages_retrieved,
    );
    insert_selected(
        &mut result,
        plan,
        ListField::PaginationRecordsRetrieved,
        "recordsRetrieved",
        pagination.records_retrieved,
    );
    insert_selected(
        &mut result,
        plan,
        ListField::PaginationRecordsReturned,
        "recordsReturned",
        pagination.records_returned,
    );
    insert_selected(
        &mut result,
        plan,
        ListField::PaginationNext,
        "next",
        &pagination.next,
    );
    insert_selected(
        &mut result,
        plan,
        ListField::PaginationComplete,
        "complete",
        pagination.complete,
    );
    insert_selected(
        &mut result,
        plan,
        ListField::PaginationTotalsComplete,
        "totalsComplete",
        pagination.totals_complete,
    );
    serde_json::Value::Object(result)
}

fn insert_selected<T: serde::Serialize>(
    result: &mut serde_json::Map<String, serde_json::Value>,
    plan: &ListProjectionPlan,
    field: ListField,
    name: &str,
    value: T,
) {
    if plan.includes(field) {
        result.insert(name.to_owned(), json!(value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_plan_derives_worklog_enrichment_requirements() -> Result<(), FieldSelectionError>
    {
        let tempo_only = ListProjectionPlan::selected(ListFieldMask::parse("worklogs.id")?);
        assert!(tempo_only.selects_worklogs());
        assert!(!tempo_only.requires_issue_key());
        assert!(!tempo_only.requires_link());
        assert!(!tempo_only.requires_interval());

        let interval = ListProjectionPlan::selected(ListFieldMask::parse("worklogs.interval")?);
        assert!(interval.requires_interval());
        assert!(!interval.requires_issue_key());

        let link = ListProjectionPlan::selected(ListFieldMask::parse("worklogs.link")?);
        assert!(link.requires_issue_key());
        assert!(link.requires_link());

        let pagination = ListProjectionPlan::selected(ListFieldMask::parse("pagination.next")?);
        assert!(!pagination.selects_worklogs());
        Ok(())
    }
}
