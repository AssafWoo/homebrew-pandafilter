//! Zoom-In block registry.
//!
//! When zoom is enabled by the calling layer, collapse/omission markers in
//! pipeline output include a `ccr expand ZI_N` reference. The original lines
//! for each block are registered here and later drained by the caller for
//! persistence to disk.

use std::cell::RefCell;

pub struct ZoomBlock {
    pub id: String,
    pub lines: Vec<String>,
}

thread_local! {
    static ENABLED: RefCell<bool> = RefCell::new(false);
    static COUNTER: RefCell<usize> = RefCell::new(0);
    static BLOCKS: RefCell<Vec<ZoomBlock>> = RefCell::new(Vec::new());
}

/// Enable zoom for the current thread. Resets the counter and block list.
/// Call this before invoking the pipeline when you have session context.
pub fn enable() {
    ENABLED.with(|e| *e.borrow_mut() = true);
    COUNTER.with(|c| *c.borrow_mut() = 0);
    BLOCKS.with(|b| b.borrow_mut().clear());
}

/// Disable zoom. Used by callers without session context (e.g. `ccr filter`).
pub fn disable() {
    ENABLED.with(|e| *e.borrow_mut() = false);
}

/// Returns true if zoom is currently enabled on this thread.
pub fn is_enabled() -> bool {
    ENABLED.with(|e| *e.borrow())
}

/// Register the original lines for a collapsed/omitted block and return a zoom ID.
/// The ID is embedded in the output marker so the user can run `ccr expand ZI_N`.
pub fn register(lines: Vec<String>) -> String {
    let id = COUNTER.with(|c| {
        let mut n = c.borrow_mut();
        *n += 1;
        format!("ZI_{}", n)
    });
    BLOCKS.with(|b| {
        b.borrow_mut().push(ZoomBlock { id: id.clone(), lines });
    });
    id
}

/// Drain all registered blocks, returning them to the caller for persistence.
/// Resets the block list. Counter is NOT reset — IDs remain unique within a session.
pub fn drain() -> Vec<ZoomBlock> {
    BLOCKS.with(|b| std::mem::take(&mut *b.borrow_mut()))
}
