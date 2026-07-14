//! Core, I/O-independent behavior for the Drag CLI.

pub mod models;
pub mod schedule;
pub mod time;
pub mod tracker;

use thiserror::Error;

/// Domain-level validation failures.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// A duration or interval could not be parsed.
    #[error("cannot parse duration or interval '{0}'")]
    InvalidDuration(String),
    /// A date selector could not be parsed.
    #[error("cannot parse '{0}' as a date; use YYYY-MM-DD, y, yesterday, t±N, or today±N")]
    InvalidDate(String),
    /// A clock value could not be parsed.
    #[error("cannot parse '{0}' as a time; use HH:mm")]
    InvalidTime(String),
    /// A parsed duration is not valid for a worklog.
    #[error("worklog duration must be greater than zero")]
    NonPositiveDuration,
    /// An operation cannot be performed in the tracker's current state.
    #[error("{0}")]
    Tracker(String),
}

impl Error {
    /// Stable error identifier for structured CLI output.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidDuration(_) => "invalid_duration",
            Self::InvalidDate(_) => "invalid_date",
            Self::InvalidTime(_) => "invalid_time",
            Self::NonPositiveDuration => "non_positive_duration",
            Self::Tracker(_) => "tracker_error",
        }
    }
}
