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
    #[serde(skip)]
    #[schemars(skip)]
    pub seconds: ScheduleSeconds,
}

/// Numeric schedule facts retained for interactive presentation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScheduleSeconds {
    pub month_required: i64,
    pub month_logged: i64,
    pub month_balance: i64,
    pub day_required: i64,
    pub day_logged: i64,
}

/// Incremental month/day totals for paginated worklog retrieval.
pub struct ScheduleAccumulator {
    month_required: i64,
    required_through_today: i64,
    day_required: i64,
    month_logged: i64,
    day_logged: i64,
    selected_date: String,
    account_id: String,
}

impl ScheduleAccumulator {
    /// Initializes the fixed schedule totals for a selected day.
    #[must_use]
    pub fn new(
        schedule: &[ScheduleEntity],
        selected_date: NaiveDate,
        today: NaiveDate,
        account_id: &str,
    ) -> Self {
        let selected_date = selected_date.to_string();
        Self {
            month_required: schedule.iter().map(|day| day.required_seconds).sum(),
            required_through_today: schedule
                .iter()
                .filter(|day| {
                    NaiveDate::parse_from_str(&day.date, "%Y-%m-%d").is_ok_and(|date| date <= today)
                })
                .map(|day| day.required_seconds)
                .sum(),
            day_required: schedule
                .iter()
                .filter(|day| day.date == selected_date)
                .map(|day| day.required_seconds)
                .sum(),
            month_logged: 0,
            day_logged: 0,
            selected_date,
            account_id: account_id.to_owned(),
        }
    }

    /// Adds one retrieved page of worklogs.
    pub fn add_worklogs(&mut self, worklogs: &[WorklogEntity]) {
        for worklog in worklogs
            .iter()
            .filter(|worklog| worklog.author.account_id == self.account_id)
        {
            self.month_logged += worklog.time_spent_seconds;
            if worklog.start_date == self.selected_date {
                self.day_logged += worklog.time_spent_seconds;
            }
        }
    }

    /// Formats the accumulated totals as a list schedule summary.
    #[must_use]
    pub fn finish(self) -> ScheduleDetails {
        let month_balance = self.month_logged - self.required_through_today;
        ScheduleDetails {
            month_required_duration: format_duration(self.month_required, false),
            month_logged_duration: format_duration(self.month_logged, false),
            month_current_period_duration: format_duration(month_balance, true),
            day_required_duration: format_duration(self.day_required, false),
            day_logged_duration: format_duration(self.day_logged, false),
            seconds: ScheduleSeconds {
                month_required: self.month_required,
                month_logged: self.month_logged,
                month_balance,
                day_required: self.day_required,
                day_logged: self.day_logged,
            },
        }
    }
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
    let mut accumulator = ScheduleAccumulator::new(schedule, selected_date, today, account_id);
    accumulator.add_worklogs(worklogs);
    accumulator.finish()
}

#[cfg(test)]
mod tests {
    use crate::models::{Author, Issue, ScheduleEntity, WorklogEntity};
    use chrono::NaiveDate;

    use super::{create_schedule_details, ScheduleAccumulator};

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
            attributes: Default::default(),
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

    #[test]
    fn incremental_schedule_totals_match_a_complete_report() -> Result<(), &'static str> {
        let worklogs = [
            WorklogEntity {
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
                time_spent_seconds: 3_600,
                attributes: Default::default(),
            },
            WorklogEntity {
                tempo_worklog_id: "2".to_owned(),
                start_date: "2020-02-04".to_owned(),
                start_time: "09:00:00".to_owned(),
                author: Author {
                    account_id: "me".to_owned(),
                },
                issue: Issue {
                    self_url: "https://example.atlassian.net/issue/2".to_owned(),
                    id: "2".to_owned(),
                },
                description: String::new(),
                time_spent_seconds: 7_200,
                attributes: Default::default(),
            },
        ];
        let schedule = [ScheduleEntity {
            date: "2020-02-03".to_owned(),
            required_seconds: 7_200,
            kind: "WORKING_DAY".to_owned(),
        }];
        let selected = NaiveDate::from_ymd_opt(2020, 2, 3).ok_or("invalid test date")?;
        let expected = create_schedule_details(&worklogs, &schedule, selected, selected, "me");
        let mut accumulator = ScheduleAccumulator::new(&schedule, selected, selected, "me");

        accumulator.add_worklogs(&worklogs[..1]);
        accumulator.add_worklogs(&worklogs[1..]);

        assert_eq!(accumulator.finish(), expected);
        Ok::<_, &'static str>(())
    }
}
