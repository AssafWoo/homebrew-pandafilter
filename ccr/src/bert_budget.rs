use std::cell::Cell;

/// Maximum number of `embed_batch` calls allowed per hook invocation.
///
/// Rationale:
/// - Normal hot path (Bash, ~100 lines): 2-3 calls (noise filter + summarize)
/// - Session update adds 1 call (now guarded by BERT_MIN_LINES=30)
/// - Budget of 6 covers all normal paths with 2 slots to spare for large inputs.
/// - Reduced from 12 → 6 to cap worst-case IPC round trips to the embedding daemon.
pub const MAX_BERT_CALLS: usize = 6;

thread_local! {
    static CALLS_REMAINING: Cell<usize> = Cell::new(MAX_BERT_CALLS);
}

/// Reset the budget counter. Must be called once at the start of each hook invocation
/// before any embed_batch call can occur.
pub fn reset() {
    CALLS_REMAINING.with(|c| c.set(MAX_BERT_CALLS));
}

/// Attempt to consume one BERT call from the budget.
/// Returns `true` if the call is permitted (budget decremented).
/// Returns `false` if the budget is exhausted; the caller must use the fallback path.
/// Never panics.
pub fn try_consume() -> bool {
    CALLS_REMAINING.with(|c| {
        let remaining = c.get();
        if remaining > 0 {
            c.set(remaining - 1);
            true
        } else {
            false
        }
    })
}

/// Returns how many BERT calls are still available in the current invocation.
/// Used by loop callers that want to pass remaining budget to a section loop (e.g. WebFetch).
pub fn remaining() -> usize {
    CALLS_REMAINING.with(|c| c.get())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_budget_allows_max_calls() {
        reset();
        for _ in 0..MAX_BERT_CALLS {
            assert!(try_consume());
        }
    }

    #[test]
    fn exhausted_budget_returns_false() {
        reset();
        for _ in 0..MAX_BERT_CALLS {
            try_consume();
        }
        assert!(!try_consume());
    }

    #[test]
    fn reset_restores_full_budget() {
        reset();
        for _ in 0..MAX_BERT_CALLS {
            try_consume();
        }
        reset();
        assert_eq!(remaining(), MAX_BERT_CALLS);
    }

    #[test]
    fn try_consume_decrements_remaining() {
        reset();
        let before = remaining();
        assert!(try_consume());
        assert_eq!(remaining(), before - 1);
    }
}
