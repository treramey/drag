//! Worklog schedule summaries.

use chrono::NaiveDate;
use schemars::JsonSchema;
use serde::Serialize;

use crate::{
    models::{ScheduleEntity, WorklogEntity},
    time::format_duration,
};

/// Month/day duration totals shown by `drag list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleDetails {
    pub month_required_duration: String,
    pub month_logged_duration: String,
    pub month_current_period_duration: String,
    pub day_required_duration: String,
    pub day_logged_duration: String,
}

/// Calculate month and day schedule details.
#[must_use]
pub fn create_schedule_details(
    worklogs: &[WorklogEntity],
    schedule: &[ScheduleEntity],
    selected_date: NaiveDate,
    today: NaiveDate,
    account_id: &str,
) -> ScheduleDetails {
    let own_worklogs: Vec<_> = worklogs
        .iter()
        .filter(|worklog| worklog.author.account_id == account_id)
        .collect();
    let month_required: i64 = schedule.iter().map(|day| day.required_seconds).sum();
    let month_logged: i64 = own_worklogs
        .iter()
        .map(|worklog| worklog.time_spent_seconds)
        .sum();
    let required_through_today: i64 = schedule
        .iter()
        .filter(|day| {
            NaiveDate::parse_from_str(&day.date, "%Y-%m-%d").is_ok_and(|date| date <= today)
        })
        .map(|day| day.required_seconds)
        .sum();
    let day_required: i64 = schedule
        .iter()
        .filter(|day| day.date == selected_date.to_string())
        .map(|day| day.required_seconds)
        .sum();
    let day_logged: i64 = own_worklogs
        .iter()
        .filter(|worklog| worklog.start_date == selected_date.to_string())
        .map(|worklog| worklog.time_spent_seconds)
        .sum();

    ScheduleDetails {
        month_required_duration: format_duration(month_required, false),
        month_logged_duration: format_duration(month_logged, false),
        month_current_period_duration: format_duration(month_logged - required_through_today, true),
        day_required_duration: format_duration(day_required, false),
        day_logged_duration: format_duration(day_logged, false),
    }
}

#[cfg(test)]
mod tests {
    use crate::models::{Author, Issue, ScheduleEntity, WorklogEntity};

    use super::create_schedule_details;

    #[test]
    fn calculates_month_and_day_balances() {
        let worklogs = vec![WorklogEntity {
            tempo_worklog_id: "1".to_owned(),
            start_date: "2020-02-03".to_owned(),
            start_time: "09:00:00".to_owned(),
            author: Author {
                account_id: "me".to_owned(),
            },
            issue: Issue {
                self_url: "https://example.atlassian.net/issue/1".to_owned(),
                id: "1".to_owned(),
            },
            description: String::new(),
            time_spent_seconds: 21_600,
        }];
        let schedule = vec![
            ScheduleEntity {
                date: "2020-02-03".to_owned(),
                required_seconds: 21_600,
                kind: "WORKING_DAY".to_owned(),
            },
            ScheduleEntity {
                date: "2020-02-04".to_owned(),
                required_seconds: 21_600,
                kind: "WORKING_DAY".to_owned(),
            },
        ];
        let selected = chrono::NaiveDate::from_ymd_opt(2020, 2, 3);
        let Some(selected) = selected else { return };
        let details = create_schedule_details(&worklogs, &schedule, selected, selected, "me");
        assert_eq!(details.month_required_duration, "12h");
        assert_eq!(details.month_logged_duration, "6h");
        assert_eq!(details.month_current_period_duration, "0h");
        assert_eq!(details.day_logged_duration, "6h");
    }
}
