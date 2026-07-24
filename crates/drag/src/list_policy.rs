//! Deterministic policy for interactive `drag list` sessions.
//!
//! This module intentionally contains only pure decisions that can be shared by
//! frontends. Network loading, async task orchestration, terminal input, and
//! rendering stay in the CLI layer.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{Datelike, Days, NaiveDate};

use crate::schedule::ScheduleDetails;

/// Cache key for month-wide schedule summaries.
pub type MonthSummaryCache = BTreeMap<(i32, u32), MonthSummary>;

/// Stable month-wide schedule fields captured from the first complete report for a month.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonthSummary {
    required_duration: String,
    logged_duration: String,
    balance_duration: String,
    required_seconds: i64,
    logged_seconds: i64,
    balance_seconds: i64,
}

impl MonthSummary {
    /// Capture only the month-wide fields from a schedule summary.
    #[must_use]
    pub fn from_schedule(details: &ScheduleDetails) -> Self {
        Self {
            required_duration: details.month_required_duration.clone(),
            logged_duration: details.month_logged_duration.clone(),
            balance_duration: details.month_current_period_duration.clone(),
            required_seconds: details.seconds.month_required,
            logged_seconds: details.seconds.month_logged,
            balance_seconds: details.seconds.month_balance,
        }
    }

    /// Apply captured month-wide fields without changing day-specific fields.
    pub fn apply_to(&self, details: &mut ScheduleDetails) {
        details
            .month_required_duration
            .clone_from(&self.required_duration);
        details
            .month_logged_duration
            .clone_from(&self.logged_duration);
        details
            .month_current_period_duration
            .clone_from(&self.balance_duration);
        details.seconds.month_required = self.required_seconds;
        details.seconds.month_logged = self.logged_seconds;
        details.seconds.month_balance = self.balance_seconds;
    }
}

/// Reuse the first complete month summary seen in an interactive session.
pub fn stabilize_month_summary(
    summaries: &mut MonthSummaryCache,
    selected_date: NaiveDate,
    totals_complete: bool,
    details: &mut ScheduleDetails,
) {
    if !totals_complete {
        return;
    }
    let key = (selected_date.year(), selected_date.month());
    if let Some(summary) = summaries.get(&key) {
        summary.apply_to(details);
    } else {
        summaries.insert(key, MonthSummary::from_schedule(details));
    }
}

/// Date navigation action emitted by an interactive list frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateAction {
    /// End the interactive list session.
    Close,
    /// Move to the previous calendar date.
    PreviousDate,
    /// Move to the next calendar date.
    NextDate,
}

/// Pure result of resolving a date navigation action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateActionResolution {
    /// The frontend requested that the session close.
    Close,
    /// The frontend requested that this date be selected.
    Date(NaiveDate),
    /// The requested movement would exceed `chrono`'s supported date range.
    OutOfRange,
}

/// Resolve a list date navigation action relative to the current selected date.
#[must_use]
pub fn resolve_date_action(date: NaiveDate, action: DateAction) -> DateActionResolution {
    let date = match action {
        DateAction::Close => return DateActionResolution::Close,
        DateAction::PreviousDate => date.checked_sub_days(Days::new(1)),
        DateAction::NextDate => date.checked_add_days(Days::new(1)),
    };
    date.map_or(DateActionResolution::OutOfRange, DateActionResolution::Date)
}

/// Cached interactive report data plus whether it is safe to display again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedReport<T> {
    /// Cached report payload.
    pub report: T,
    /// Whether the payload may be reused for a later visit to the same date.
    pub reusable: bool,
}

/// Remove and return the cached report for `date` only when it is marked reusable.
pub fn take_reusable_report<T>(
    reports: &mut BTreeMap<NaiveDate, CachedReport<T>>,
    date: NaiveDate,
) -> Option<T> {
    if !reports.get(&date).is_some_and(|cached| cached.reusable) {
        return None;
    }
    reports.remove(&date).map(|cached| cached.report)
}

/// Return the previous and next date around `date`, or `None` at supported range boundaries.
#[must_use]
pub fn adjacent_dates(date: NaiveDate) -> Option<[NaiveDate; 2]> {
    let previous = date.checked_sub_days(Days::new(1))?;
    let next = date.checked_add_days(Days::new(1))?;
    Some([previous, next])
}

/// Choose adjacent dates that are not already cached or being prefetched.
#[must_use]
pub fn dates_to_prefetch(
    selected_date: NaiveDate,
    cached_dates: impl IntoIterator<Item = NaiveDate>,
    pending_dates: impl IntoIterator<Item = NaiveDate>,
) -> Option<Vec<NaiveDate>> {
    let cached_dates: BTreeSet<_> = cached_dates.into_iter().collect();
    let pending_dates: BTreeSet<_> = pending_dates.into_iter().collect();
    Some(
        adjacent_dates(selected_date)?
            .into_iter()
            .filter(|date| !cached_dates.contains(date) && !pending_dates.contains(date))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::NaiveDate;

    use super::{
        adjacent_dates, dates_to_prefetch, resolve_date_action, stabilize_month_summary,
        take_reusable_report, CachedReport, DateAction, DateActionResolution, MonthSummaryCache,
    };
    use crate::schedule::{ScheduleDetails, ScheduleSeconds};

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap_or(NaiveDate::MIN)
    }

    fn schedule_details(
        month_logged: &str,
        day_logged: &str,
        month_logged_seconds: i64,
        day_logged_seconds: i64,
    ) -> ScheduleDetails {
        ScheduleDetails {
            month_required_duration: "176h".to_owned(),
            month_logged_duration: month_logged.to_owned(),
            month_current_period_duration: "-14h".to_owned(),
            day_required_duration: "8h".to_owned(),
            day_logged_duration: day_logged.to_owned(),
            seconds: ScheduleSeconds {
                month_required: 176 * 3_600,
                month_logged: month_logged_seconds,
                month_balance: -14 * 3_600,
                day_required: 8 * 3_600,
                day_logged: day_logged_seconds,
            },
        }
    }

    #[test]
    fn month_summary_reuses_first_complete_summary_for_same_month() {
        let mut summaries = MonthSummaryCache::new();
        let mut first = schedule_details("106h", "1h", 106 * 3_600, 3_600);
        let mut next = schedule_details("107h", "0h", 107 * 3_600, 0);

        stabilize_month_summary(&mut summaries, date(2026, 7, 21), true, &mut first);
        stabilize_month_summary(&mut summaries, date(2026, 7, 22), true, &mut next);

        assert_eq!(next.month_logged_duration, "106h");
        assert_eq!(next.seconds.month_logged, 106 * 3_600);
        assert_eq!(next.day_logged_duration, "0h");
        assert_eq!(next.seconds.day_logged, 0);
    }

    #[test]
    fn month_summary_ignores_incomplete_totals() {
        let mut summaries = MonthSummaryCache::new();
        let mut incomplete = schedule_details("50h", "1h", 50 * 3_600, 3_600);
        let mut complete = schedule_details("106h", "0h", 106 * 3_600, 0);

        stabilize_month_summary(&mut summaries, date(2026, 7, 21), false, &mut incomplete);
        stabilize_month_summary(&mut summaries, date(2026, 7, 22), true, &mut complete);

        assert_eq!(complete.month_logged_duration, "106h");
        assert_eq!(summaries.len(), 1);
    }

    #[test]
    fn resolves_date_actions_to_close_previous_and_next() {
        let selected = date(2026, 7, 21);

        assert_eq!(
            resolve_date_action(selected, DateAction::Close),
            DateActionResolution::Close
        );
        assert_eq!(
            resolve_date_action(selected, DateAction::PreviousDate),
            DateActionResolution::Date(date(2026, 7, 20))
        );
        assert_eq!(
            resolve_date_action(selected, DateAction::NextDate),
            DateActionResolution::Date(date(2026, 7, 22))
        );
    }

    #[test]
    fn date_action_reports_out_of_range_at_boundaries() {
        assert_eq!(
            resolve_date_action(NaiveDate::MIN, DateAction::PreviousDate),
            DateActionResolution::OutOfRange
        );
        assert_eq!(
            resolve_date_action(NaiveDate::MAX, DateAction::NextDate),
            DateActionResolution::OutOfRange
        );
    }

    #[test]
    fn cache_selection_removes_only_reusable_reports() {
        let selected = date(2026, 7, 21);
        let stale = date(2026, 7, 22);
        let mut reports = BTreeMap::from([
            (
                selected,
                CachedReport {
                    report: "selected",
                    reusable: true,
                },
            ),
            (
                stale,
                CachedReport {
                    report: "stale",
                    reusable: false,
                },
            ),
        ]);

        assert_eq!(
            take_reusable_report(&mut reports, selected),
            Some("selected")
        );
        assert_eq!(take_reusable_report(&mut reports, stale), None);
        assert!(!reports.contains_key(&selected));
        assert!(reports.contains_key(&stale));
    }

    #[test]
    fn adjacent_date_policy_returns_previous_and_next_or_boundary_none() {
        assert_eq!(
            adjacent_dates(date(2026, 7, 21)),
            Some([date(2026, 7, 20), date(2026, 7, 22)])
        );
        assert_eq!(adjacent_dates(NaiveDate::MIN), None);
        assert_eq!(adjacent_dates(NaiveDate::MAX), None);
    }

    #[test]
    fn adjacent_prefetch_policy_skips_cached_and_pending_dates() {
        let selected = date(2026, 7, 21);
        let cached = [date(2026, 7, 20)];
        let pending = [date(2026, 7, 23)];

        assert_eq!(
            dates_to_prefetch(selected, cached, pending),
            Some(vec![date(2026, 7, 22)])
        );
    }
}
