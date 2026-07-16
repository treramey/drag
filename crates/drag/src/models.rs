//! Shared API and presentation models.

use serde::{Deserialize, Deserializer, Serialize};

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
    #[serde(deserialize_with = "deserialize_string_or_u64")]
    pub tempo_worklog_id: String,
    pub start_date: String,
    pub start_time: String,
    pub author: Author,
    pub issue: Issue,
    #[serde(default)]
    pub description: String,
    pub time_spent_seconds: i64,
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
}
