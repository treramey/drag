//! Deterministic bounds for collection traversal.

use thiserror::Error;

/// Default maximum number of records returned by a bounded collection read.
pub const DEFAULT_RECORD_LIMIT: usize = 100;
/// Default maximum number of pages retrieved by a bounded collection read.
pub const DEFAULT_PAGE_LIMIT: u16 = 1;
/// Maximum page count even when exhaustive traversal is explicitly requested.
pub const HARD_PAGE_LIMIT: u16 = 100;
/// Largest caller-selected record limit accepted by the CLI.
pub const MAX_RECORD_LIMIT: usize = 1_000;

/// Invalid bounded traversal settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("pagination limits must be within the supported safety bounds")]
pub struct PaginationError;

/// Why a traversal could not safely account for another page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TraversalError {
    /// An exhaustive traversal received a continuation at the hard page ceiling.
    #[error("pagination exceeded the hard page limit")]
    HardPageLimitExceeded,
    /// Page or record accounting exceeded its representable range.
    #[error("pagination accounting overflowed")]
    AccountingOverflow,
}

/// The deterministic action to take after consuming a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraversalDecision {
    /// The remote service reported a terminal page.
    Complete,
    /// The traversal remains within its configured bounds.
    Continue,
    /// A continuation exists, but the configured record or page bound was reached.
    Bounded,
}

/// Pure traversal policy shared by collection workflows and HTTP adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaginationPlan {
    record_limit: Option<usize>,
    page_limit: u16,
}

impl PaginationPlan {
    pub const fn bounded(record_limit: usize, page_limit: u16) -> Result<Self, PaginationError> {
        if record_limit == 0
            || record_limit > MAX_RECORD_LIMIT
            || page_limit == 0
            || page_limit > HARD_PAGE_LIMIT
        {
            return Err(PaginationError);
        }
        Ok(Self {
            record_limit: Some(record_limit),
            page_limit,
        })
    }

    #[must_use]
    pub const fn all_pages() -> Self {
        Self {
            record_limit: None,
            page_limit: HARD_PAGE_LIMIT,
        }
    }

    #[must_use]
    pub const fn record_limit(self) -> Option<usize> {
        self.record_limit
    }

    #[must_use]
    pub const fn page_limit(self) -> u16 {
        self.page_limit
    }

    #[must_use]
    pub const fn is_all_pages(self) -> bool {
        self.record_limit.is_none()
    }

    #[must_use]
    pub fn request_limit(self, records_retrieved: usize) -> usize {
        self.record_limit.map_or(DEFAULT_RECORD_LIMIT, |limit| {
            limit.saturating_sub(records_retrieved)
        })
    }

    #[must_use]
    pub fn should_follow(self, pages_retrieved: u16, records_retrieved: usize) -> bool {
        pages_retrieved < self.page_limit
            && self
                .record_limit
                .is_none_or(|limit| records_retrieved < limit)
    }
}

/// Page and record accounting for one pagination segment.
///
/// The state is intentionally independent of continuation URLs and page payloads so callers keep
/// authentication, URL validation, parsing, and output at their I/O boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraversalState {
    plan: PaginationPlan,
    pages_retrieved: u16,
    records_retrieved: usize,
}

impl TraversalState {
    #[must_use]
    pub const fn new(plan: PaginationPlan) -> Self {
        Self {
            plan,
            pages_retrieved: 0,
            records_retrieved: 0,
        }
    }

    /// Account for a fetched page and decide whether its continuation should be followed.
    pub fn consume_page(
        &mut self,
        page_count: u16,
        record_count: usize,
        has_next: bool,
    ) -> Result<TraversalDecision, TraversalError> {
        self.pages_retrieved = self
            .pages_retrieved
            .checked_add(page_count)
            .ok_or(TraversalError::AccountingOverflow)?;
        self.records_retrieved = self
            .records_retrieved
            .checked_add(record_count)
            .ok_or(TraversalError::AccountingOverflow)?;

        if !has_next {
            return Ok(TraversalDecision::Complete);
        }
        if self
            .plan
            .should_follow(self.pages_retrieved, self.records_retrieved)
        {
            return Ok(TraversalDecision::Continue);
        }
        if self.plan.is_all_pages() && self.pages_retrieved >= HARD_PAGE_LIMIT {
            return Err(TraversalError::HardPageLimitExceeded);
        }
        Ok(TraversalDecision::Bounded)
    }

    #[must_use]
    pub const fn pages_retrieved(self) -> u16 {
        self.pages_retrieved
    }

    #[must_use]
    pub const fn records_retrieved(self) -> usize {
        self.records_retrieved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_plan_stops_at_either_record_or_page_limit() -> Result<(), PaginationError> {
        let plan = PaginationPlan::bounded(25, 3)?;

        assert_eq!(plan.request_limit(0), 25);
        assert!(plan.should_follow(1, 10));
        assert!(!plan.should_follow(1, 25));
        assert!(!plan.should_follow(3, 10));
        Ok(())
    }

    #[test]
    fn all_pages_plan_uses_finite_pages_and_a_hard_ceiling() {
        let plan = PaginationPlan::all_pages();

        assert_eq!(plan.request_limit(0), DEFAULT_RECORD_LIMIT);
        assert!(plan.should_follow(HARD_PAGE_LIMIT - 1, 10_000));
        assert!(!plan.should_follow(HARD_PAGE_LIMIT, 10_000));
    }

    #[test]
    fn bounded_plan_rejects_values_outside_the_public_safety_limits() {
        for (record_limit, page_limit) in [
            (0, 1),
            (MAX_RECORD_LIMIT + 1, 1),
            (1, 0),
            (1, HARD_PAGE_LIMIT + 1),
        ] {
            assert!(PaginationPlan::bounded(record_limit, page_limit).is_err());
        }
    }

    #[test]
    fn traversal_accounts_for_empty_final_and_bounded_pages() -> Result<(), TraversalError> {
        let mut state = TraversalState::new(
            PaginationPlan::bounded(3, 2).map_err(|_| TraversalError::AccountingOverflow)?,
        );

        assert_eq!(state.consume_page(1, 0, true)?, TraversalDecision::Continue);
        assert_eq!(state.consume_page(1, 3, true)?, TraversalDecision::Bounded);
        assert_eq!(state.pages_retrieved(), 2);
        assert_eq!(state.records_retrieved(), 3);

        let mut final_page = TraversalState::new(PaginationPlan::all_pages());
        assert_eq!(
            final_page.consume_page(1, 0, false)?,
            TraversalDecision::Complete
        );
        Ok(())
    }

    #[test]
    fn all_pages_accepts_an_exact_terminal_ceiling_but_rejects_a_continuation() {
        let mut terminal = TraversalState::new(PaginationPlan::all_pages());
        assert_eq!(
            terminal.consume_page(HARD_PAGE_LIMIT, 1, false),
            Ok(TraversalDecision::Complete)
        );

        let mut overflow = TraversalState::new(PaginationPlan::all_pages());
        assert_eq!(
            overflow.consume_page(HARD_PAGE_LIMIT, 1, true),
            Err(TraversalError::HardPageLimitExceeded)
        );
    }

    #[test]
    fn each_segment_starts_fresh_for_resumed_traversal() -> Result<(), TraversalError> {
        let plan =
            PaginationPlan::bounded(10, 2).map_err(|_| TraversalError::AccountingOverflow)?;
        let mut resumed = TraversalState::new(plan);

        assert_eq!(
            resumed.consume_page(1, 4, true)?,
            TraversalDecision::Continue
        );
        assert_eq!(resumed.pages_retrieved(), 1);
        assert_eq!(resumed.records_retrieved(), 4);
        Ok(())
    }
}
