//! Shared API and presentation models.

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

/// Request body accepted by Tempo API v4 when creating a worklog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes: Option<Vec<WorkAttributeValue>>,
}

/// A Tempo work attribute value attached to a worklog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkAttributeValue {
    pub key: String,
    pub value: String,
}

/// Work attributes embedded in a Tempo worklog response.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorklogAttributes {
    #[serde(default)]
    pub values: Vec<WorkAttributeValue>,
}

/// Definition returned by Tempo's work-attributes endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkAttribute {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub required: bool,
}

/// Tempo worklog response entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorklogEntity {
    #[serde(deserialize_with = "deserialize_string_or_u64")]
    pub tempo_worklog_id: String,
    pub start_date: String,
    pub start_time: String,
    pub author: Author,
    pub issue: Issue,
    #[serde(default)]
    pub description: String,
    pub time_spent_seconds: i64,
    #[serde(default)]
    pub attributes: WorklogAttributes,
}

fn deserialize_string_or_u64<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrU64 {
        String(String),
        U64(u64),
    }

    match StringOrU64::deserialize(deserializer)? {
        StringOrU64::String(value) => Ok(value),
        StringOrU64::U64(value) => Ok(value.to_string()),
    }
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
    #[serde(deserialize_with = "deserialize_string_or_u64")]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Worklog {
    pub id: String,
    pub interval: Option<ClockInterval>,
    pub issue_id: String,
    pub issue_key: String,
    pub duration: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<WorkAttributeValue>,
    pub link: String,
}

/// Human-readable start and end clock times.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClockInterval {
    pub start_time: String,
    pub end_time: String,
}

/// Bounded traversal state emitted with list results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListPagination {
    /// Selected day to reuse when continuing this traversal.
    pub selected_date: String,
    /// Inclusive month boundary embedded in the continuation.
    pub month_start: String,
    /// Inclusive month boundary embedded in the continuation.
    pub month_end: String,
    #[schemars(range(min = 1, max = 1_000))]
    pub limit: Option<usize>,
    #[schemars(range(min = 1, max = 100))]
    pub page_limit: u16,
    pub all_pages: bool,
    #[schemars(range(min = 1, max = 100))]
    pub pages_retrieved: u16,
    pub records_retrieved: usize,
    pub records_returned: usize,
    /// Opaque Drag token accepted by `list --continue-from`.
    pub next: Option<String>,
    /// Whether this segment reached a terminal Tempo page.
    pub complete: bool,
    /// Whether schedule totals include the entire selected month.
    pub totals_complete: bool,
}

#[cfg(test)]
mod tests {
    use super::WorklogEntity;

    fn worklog_json(worklog_id: &str, issue_id: &str) -> String {
        format!(
            r#"{{
                "tempoWorklogId": {worklog_id},
                "startDate": "2026-07-14",
                "startTime": "09:00:00",
                "author": {{"accountId": "me"}},
                "issue": {{"self": "https://example.atlassian.net/issue/1", "id": {issue_id}}},
                "timeSpentSeconds": 3600
            }}"#
        )
    }

    #[test]
    fn worklog_entity_accepts_numeric_tempo_id() -> Result<(), serde_json::Error> {
        let entity: WorklogEntity = serde_json::from_str(&worklog_json("751393", r#""1""#))?;

        assert_eq!(entity.tempo_worklog_id, "751393");
        Ok(())
    }

    #[test]
    fn worklog_entity_preserves_string_tempo_id() -> Result<(), serde_json::Error> {
        let entity: WorklogEntity = serde_json::from_str(&worklog_json(r#""751393""#, r#""1""#))?;

        assert_eq!(entity.tempo_worklog_id, "751393");
        Ok(())
    }

    #[test]
    fn worklog_entity_accepts_numeric_issue_id() -> Result<(), serde_json::Error> {
        let entity: WorklogEntity = serde_json::from_str(&worklog_json("751393", "275038"))?;

        assert_eq!(entity.issue.id, "275038");
        Ok(())
    }

    #[test]
    fn worklog_entity_preserves_embedded_attributes() -> Result<(), serde_json::Error> {
        let json = worklog_json("751393", "275038").replace(
            r#""timeSpentSeconds": 3600"#,
            r#""timeSpentSeconds": 3600,
                "attributes": {"values": [
                    {"key": "_Worktype_", "value": "Development"},
                    {"key": "_Billable_", "value": "true"}
                ]}"#,
        );
        let entity: WorklogEntity = serde_json::from_str(&json)?;

        assert_eq!(entity.attributes.values.len(), 2);
        assert_eq!(entity.attributes.values[0].key, "_Worktype_");
        assert_eq!(entity.attributes.values[0].value, "Development");
        Ok(())
    }
}
