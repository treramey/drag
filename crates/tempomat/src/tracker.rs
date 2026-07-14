//! Persistent tracker state machine.

use serde::{Deserialize, Serialize};

/// One completed active period, represented as Unix epoch milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerInterval {
    pub start: i64,
    pub end: i64,
}

/// A local issue timer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tracker {
    pub issue_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub active_timestamp: i64,
    pub is_active: bool,
    #[serde(default)]
    pub intervals: Vec<TrackerInterval>,
}

impl Tracker {
    /// Create an active tracker.
    #[must_use]
    pub fn new(issue_key: String, description: Option<String>, now_millis: i64) -> Self {
        Self {
            issue_key,
            description,
            active_timestamp: now_millis,
            is_active: true,
            intervals: Vec::new(),
        }
    }

    /// Pause a tracker, retaining active periods of at least one minute.
    pub fn pause(&mut self, now_millis: i64) {
        if self.is_active {
            if now_millis - self.active_timestamp >= 60_000 {
                self.intervals.push(TrackerInterval {
                    start: self.active_timestamp,
                    end: now_millis,
                });
            }
            self.is_active = false;
        }
    }

    /// Resume a paused tracker. Resuming an active tracker is idempotent.
    pub fn resume(&mut self, now_millis: i64) {
        if !self.is_active {
            self.active_timestamp = now_millis;
        }
        self.is_active = true;
    }

    /// Pause and optionally replace the description before upload.
    pub fn stop(&mut self, now_millis: i64, description: Option<String>) {
        self.pause(now_millis);
        if description.is_some() {
            self.description = description;
        }
    }

    /// Total complete minutes, including the current active period.
    #[must_use]
    pub fn total_minutes(&self, now_millis: i64) -> i64 {
        let completed: i64 = self
            .intervals
            .iter()
            .map(|interval| (interval.end - interval.start) / 60_000)
            .sum();
        let active = if self.is_active {
            (now_millis - self.active_timestamp) / 60_000
        } else {
            0
        };
        completed + active
    }
}

#[cfg(test)]
mod tests {
    use super::Tracker;

    #[test]
    fn pause_resume_state_matches_original() {
        let mut tracker = Tracker::new("ABC-1".to_owned(), None, 0);
        tracker.pause(59_000);
        assert!(tracker.intervals.is_empty());
        tracker.resume(60_000);
        tracker.pause(180_000);
        assert_eq!(tracker.intervals.len(), 1);
        assert_eq!(tracker.total_minutes(300_000), 2);
    }
}
