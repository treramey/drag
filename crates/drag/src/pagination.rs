//! Deterministic bounds for collection traversal.

/// Default maximum number of records returned by a bounded collection read.
pub const DEFAULT_RECORD_LIMIT: usize = 100;
/// Default maximum number of pages retrieved by a bounded collection read.
pub const DEFAULT_PAGE_LIMIT: u16 = 1;
/// Maximum page count even when exhaustive traversal is explicitly requested.
pub const HARD_PAGE_LIMIT: u16 = 100;
/// Largest caller-selected record limit accepted by the CLI.
pub const MAX_RECORD_LIMIT: usize = 1_000;

/// Pure traversal policy shared by collection workflows and HTTP adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaginationPlan {
    record_limit: Option<usize>,
    page_limit: u16,
}

impl PaginationPlan {
    #[must_use]
    pub const fn bounded(record_limit: usize, page_limit: u16) -> Self {
        Self {
            record_limit: Some(record_limit),
            page_limit,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_plan_stops_at_either_record_or_page_limit() {
        let plan = PaginationPlan::bounded(25, 3);

        assert_eq!(plan.request_limit(0), 25);
        assert!(plan.should_follow(1, 10));
        assert!(!plan.should_follow(1, 25));
        assert!(!plan.should_follow(3, 10));
    }

    #[test]
    fn all_pages_plan_uses_finite_pages_and_a_hard_ceiling() {
        let plan = PaginationPlan::all_pages();

        assert_eq!(plan.request_limit(0), DEFAULT_RECORD_LIMIT);
        assert!(plan.should_follow(HARD_PAGE_LIMIT - 1, 10_000));
        assert!(!plan.should_follow(HARD_PAGE_LIMIT, 10_000));
    }
}
