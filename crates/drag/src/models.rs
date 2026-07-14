//! Shared API and presentation models.

use serde::{Deserialize, Serialize};

/// Request body accepted by Tempo API v4 when creating a worklog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddWorklogRequest {
    pub issue_id: String,
    pub time_spent_seconds: i64,
    pub start_date: String,
    pub start_time: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_estimate_seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author_account_id: Option<String>,
}

/// Tempo worklog response entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorklogEntity {
    pub tempo_worklog_id: String,
    pub start_date: String,
    pub start_time: String,
    pub author: Author,
    pub issue: Issue,
    #[serde(default)]
    pub description: String,
    pub time_spent_seconds: i64,
}

/// Worklog author reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Author {
    pub account_id: String,
}

/// Worklog issue reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    #[serde(rename = "self")]
    pub self_url: String,
    pub id: String,
}

/// One day returned by Tempo's user-schedule endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleEntity {
    pub date: String,
    pub required_seconds: i64,
    #[serde(rename = "type")]
    pub kind: String,
}

/// Friendly worklog shape emitted by the CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Worklog {
    pub id: String,
    pub interval: Option<ClockInterval>,
    pub issue_id: String,
    pub issue_key: String,
    pub duration: String,
    pub description: String,
    pub link: String,
}

/// Human-readable start and end clock times.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClockInterval {
    pub start_time: String,
    pub end_time: String,
}
