// Yapfast: not part of upstream Slint. See docs/building.md in Yapfast.

//! Keeping a list's rows where they are on screen while rows go in above them, as a chat's
//! timeline does when older messages load.
//!
//! A `ListView` keeps its rows in place when rows go in above the first one it shows. Rows that go
//! in right above that first row, as when someone reads the top of a timeline, show above it
//! instead, pushing everything shown down. Inside [`hold`], they count as rows above it too, and
//! that row keeps its bottom edge where it was: its own top part, such as a date or a loading
//! notice that only the first row carries, can change with the rows before it.

use core::cell::Cell;

std::thread_local! {
    static HOLDING: Cell<bool> = const { Cell::new(false) };
}

/// Runs `change`, which adds rows to a model, keeping the rows a `ListView` shows of it in place.
pub fn hold<R>(change: impl FnOnce() -> R) -> R {
    let before = HOLDING.with(|holding| holding.replace(true));
    let result = change();
    HOLDING.with(|holding| holding.set(before));
    result
}

pub(crate) fn holding() -> bool {
    HOLDING.with(Cell::get)
}
