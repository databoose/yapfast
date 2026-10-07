// Talk: not part of upstream Slint. See docs/building.md in Talk.

//! Selecting text across the rows of a list, as a browser selects across a page.
//!
//! A press in a `StyledText` starts a selection, and dragging carries it on through the texts and
//! pictures around it: the visible ones in the same flickable, such as a list's rows, in reading
//! order. Pictures are the `Image`s with an `accessible-label`. A text outside any list is a
//! selection of its own.
//!
//! A list drops the rows scrolled out of view, so the selection doesn't live in the items: it is
//! kept as positions, each a row's index in the list's model, a part of that row in reading order,
//! and an offset in that part. Items work out what of them it covers when they're drawn, so rows
//! scrolled back into view show it again. Copying takes the partly selected rows at either end as
//! they were shown, and every row between from [`set_row_text`], which the application provides,
//! so rows that were never shown are copied too. Dragging near the top or bottom of the list
//! scrolls it. Ctrl+C copies the selection unless a text input with a selection of its own has
//! the focus, a left press anywhere clears it, and letting go copies it to the selection
//! clipboard where there is one.

use alloc::boxed::Box;
use alloc::rc::{Rc, Weak};
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::ops::Range;
use core::pin::Pin;

use crate::Property;
use crate::graphics::Color;
use crate::item_tree::{ItemRc, ItemWeak, ParentItemTraversalMode};
use crate::items::{AccessibleRole, Flickable, StyledTextItem};
use crate::lengths::{LogicalLength, LogicalPoint, LogicalRect};
use crate::platform::Clipboard;
use crate::textlayout::sharedparley::TextUnit;
use crate::timers::{Timer, TimerMode};
use crate::window::{WindowAdapter, WindowInner};

/// A place in a list: a row of its model, a part of that row in reading order, and a byte offset
/// in that part's text. A picture is one byte long.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct Position {
    row: usize,
    part: usize,
    offset: usize,
}

impl Position {
    fn row_start(row: usize) -> Self {
        Self { row, part: 0, offset: 0 }
    }

    fn row_end(row: usize) -> Self {
        Self { row, part: usize::MAX, offset: usize::MAX }
    }
}

/// Something a selection can cover: a text, or a picture.
struct Part {
    item: ItemRc,
    /// Where it is, in window coordinates.
    rect: LogicalRect,
    picture: bool,
}

impl Part {
    fn len(&self) -> usize {
        if self.picture {
            1
        } else {
            self.item.downcast::<StyledTextItem>().map_or(0, |text| text.as_pin_ref().plain().len())
        }
    }

    /// What of this part `range` covers, as copied.
    fn text(&self, range: Range<usize>) -> Option<String> {
        if self.picture {
            return Some("[Picture]".into());
        }
        let text = self.item.downcast::<StyledTextItem>()?.as_pin_ref().plain();
        text.get(range).map(String::from)
    }
}

/// The selection, from `start` to `end`, in the list `scope`.
struct Selection {
    scope: ItemWeak,
    start: Position,
    end: Position,
    /// What's copied of the rows at either end: from `start` to the end of its row, and from the
    /// start of `end`'s row to `end`. When both are in one row, `start_text` is all of it.
    start_text: String,
    end_text: String,
    color: Color,
}

/// A press selecting.
struct Drag {
    scope: ItemWeak,
    unit: TextUnit,
    /// Where the press began: a point, or the word or paragraph there.
    from: Position,
    to: Position,
    /// The press's row as copied before `to` and after `from`, for when it scrolls away.
    before: String,
    after: String,
    color: Color,
    /// The pointer, in window coordinates.
    pointer: LogicalPoint,
    window: Weak<dyn WindowAdapter>,
    /// How far the list scrolls each tick, while the pointer is near or past its edge.
    speed: f32,
    scroller: Timer,
}

std::thread_local! {
    static DRAG: RefCell<Option<Drag>> = const { RefCell::new(None) };
    static SELECTION: RefCell<Option<Selection>> = const { RefCell::new(None) };
    /// Changes whenever the selection does, so the items showing it are drawn again.
    static CHANGES: Pin<Box<Property<u32>>> = Box::pin(Property::new(0));
    static ROW_TEXT: RefCell<Option<Box<dyn Fn(usize) -> Option<String>>>> =
        const { RefCell::new(None) };
    static CLICKED_LINK: core::cell::Cell<Option<LogicalRect>> = const { core::cell::Cell::new(None) };
}

/// Where in the window the link a styled text's `link-clicked` reports is, on the line
/// clicked: read it in the callback, so a popup it opens can sit beside the link.
pub fn clicked_link() -> Option<LogicalRect> {
    CLICKED_LINK.with(core::cell::Cell::get)
}

pub(crate) fn set_clicked_link(rect: Option<LogicalRect>) {
    CLICKED_LINK.with(|clicked| clicked.set(rect));
}

/// Sets how a row of the list a selection runs through is copied when all of it is selected,
/// given its index in the list's model. It should read as the row's parts do when copied: texts
/// beside each other joined by a space, the rest by line breaks, and `[Picture]` for each
/// picture. The application has one list with selectable rows.
pub fn set_row_text(row_text: impl Fn(usize) -> Option<String> + 'static) {
    ROW_TEXT.with(|hook| *hook.borrow_mut() = Some(Box::new(row_text)));
}

/// The text of styled text as it reads, its paragraphs joined by newlines.
pub fn plain(text: &crate::styled_text::StyledText) -> String {
    crate::styled_text::get_raw_text(text).into_owned()
}

/// Makes what's being evaluated, such as an item being drawn, depend on the selection.
pub(crate) fn track() {
    CHANGES.with(|changes| changes.as_ref().get());
}

fn changed() {
    CHANGES.with(|changes| changes.as_ref().set(changes.as_ref().get_untracked().wrapping_add(1)));
}

fn is_flickable(item: &ItemRc) -> bool {
    item.downcast::<Flickable>().is_some()
}

fn window_rect(item: &ItemRc) -> LogicalRect {
    let geometry = item.geometry();
    LogicalRect::new(item.map_to_window(geometry.origin), geometry.size)
}

/// The list `item` is in, its row there and that row's index: the instance of a repeater inside a
/// flickable. Outside any list, the item is its own list and row.
fn place(item: &ItemRc) -> (ItemRc, ItemRc, usize) {
    let mut root = ItemRc::new(item.item_tree().clone(), 0);
    while let Some(parent) = root.parent_item(ParentItemTraversalMode::StopAtPopups) {
        // A flickable puts its children in a viewport of its own.
        let flickable = if is_flickable(&parent) {
            Some(parent.clone())
        } else {
            parent.parent_item(ParentItemTraversalMode::StopAtPopups).filter(is_flickable)
        };
        if let Some(flickable) = flickable {
            let index = vtable::VRc::borrow_pin(root.item_tree()).as_ref().subtree_index();
            return (flickable, root, index);
        }
        root = ItemRc::new(parent.item_tree().clone(), 0);
    }
    (item.clone(), item.clone(), 0)
}

fn is_picture(item: &ItemRc) -> bool {
    (item.downcast::<crate::items::ImageItem>().is_some()
        || item.downcast::<crate::items::ClippedImage>().is_some())
        && item.accessible_role() == AccessibleRole::Image
        // Every image has that role by default; icons have no label.
        && item
            .accessible_string_property(crate::accessibility::AccessibleStringProperty::Label)
            .is_some_and(|label| !label.is_empty())
}

/// A row's parts in reading order: top to bottom, then left to right.
fn parts(row: &ItemRc) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut add = |item: &ItemRc| {
        let picture = is_picture(item);
        if picture || item.downcast::<StyledTextItem>().is_some() {
            let rect = window_rect(item);
            if rect.width() > 0 as _ && rect.height() > 0 as _ {
                parts.push(Part { item: item.clone(), rect, picture });
            }
        }
    };
    add(row);
    row.visit_descendants(|item| {
        add(item);
        core::ops::ControlFlow::<()>::Continue(())
    });
    parts.sort_by(|a, b| {
        (a.rect.min_y(), a.rect.min_x())
            .partial_cmp(&(b.rect.min_y(), b.rect.min_x()))
            .unwrap_or(core::cmp::Ordering::Equal)
    });
    parts
}

/// What of a row's `parts` lies between `start` and `end`, as copied: texts level with each other
/// joined by a space, the rest, and pictures, by line breaks.
fn row_text(parts: &[Part], row: usize, start: Position, end: Position) -> String {
    let mut text = String::new();
    let mut last: Option<&Part> = None;
    for (index, part) in parts.iter().enumerate() {
        let Some(range) = covered_range(part.len(), row, index, start, end) else { continue };
        let Some(piece) = part.text(range) else { continue };
        if let Some(last) = last {
            let level = !part.picture
                && !last.picture
                && part.rect.min_y() < last.rect.max_y()
                && last.rect.min_y() < part.rect.max_y();
            text.push(if level { ' ' } else { '\n' });
        }
        text.push_str(&piece);
        last = Some(part);
    }
    text
}

/// The bytes of part `part` of row `row`, `len` long, between `start` and `end`.
fn covered_range(
    len: usize,
    row: usize,
    part: usize,
    start: Position,
    end: Position,
) -> Option<Range<usize>> {
    let first = Position { row, part, offset: 0 };
    let last = Position { row, part, offset: len };
    if end <= first || start >= last {
        return None;
    }
    let from = if start > first { start.offset } else { 0 };
    let to = if end < last { end.offset } else { len };
    (from < to).then_some(from..to)
}

/// Starts a selection at `position`, in `text`'s coordinates: a point, or for a double or triple
/// press the word or paragraph there. Returns whether it started.
pub(crate) fn press(
    item: &ItemRc,
    text: Pin<&StyledTextItem>,
    window_adapter: &Rc<dyn WindowAdapter>,
    position: LogicalPoint,
    click_count: u8,
) -> bool {
    clear();
    let (scope, row_root, row) = place(item);
    let parts = parts(&row_root);
    let Some(part) = parts.iter().position(|part| &part.item == item) else { return false };
    let unit = match click_count % 3 {
        0 => TextUnit::Character,
        1 => TextUnit::Word,
        _ => TextUnit::Paragraph,
    };
    let Some(range) = text.range_at(item, window_adapter, position, unit) else { return false };
    let from = Position { row, part, offset: range.start };
    let to = Position { row, part, offset: range.end };
    let color = text.link_color().with_alpha(0.35);
    let drag = Drag {
        scope: scope.downgrade(),
        unit,
        from,
        to,
        before: row_text(&parts, row, Position::row_start(row), to),
        after: row_text(&parts, row, from, Position::row_end(row)),
        color,
        pointer: window_rect(item).origin + position.to_vector(),
        window: Rc::downgrade(window_adapter),
        speed: 0.,
        scroller: Timer::default(),
    };
    if from != to {
        set(Selection {
            scope: scope.downgrade(),
            start: from,
            end: to,
            start_text: row_text(&parts, row, from, to),
            end_text: String::new(),
            color,
        });
    }
    DRAG.with(|old| *old.borrow_mut() = Some(drag));
    true
}

fn set(selection: Selection) {
    SELECTION.with(|old| *old.borrow_mut() = Some(selection));
    changed();
}

/// Clears the selection, unless a press is making one.
pub(crate) fn clear() {
    if DRAG.with(|drag| drag.borrow().is_some()) {
        return;
    }
    if SELECTION.with(|selection| selection.borrow_mut().take()).is_some() {
        changed();
    }
}

/// Whether a press is selecting in `flickable`'s rows, and if so whether it has selected
/// anything yet.
pub(crate) fn drag_in(flickable: &ItemRc) -> Option<bool> {
    let in_scope = DRAG.with(|drag| {
        drag.borrow().as_ref().is_some_and(|drag| drag.scope.upgrade().as_ref() == Some(flickable))
    });
    in_scope.then(|| SELECTION.with(|selection| selection.borrow().is_some()))
}

/// A left press anywhere ends any press before it, which may have lost the pointer without
/// letting go, as when its text is dropped, and clears the selection; a press in text then starts
/// a new one.
pub(crate) fn pressed() {
    if let Some(drag) = DRAG.with(|drag| drag.borrow_mut().take()) {
        drag.scroller.stop();
    }
    clear();
}

/// The visible parts of the list, by row, each row with all its parts so they're numbered as
/// they are when drawn.
fn rows_in(scope: &ItemRc) -> Vec<(usize, Vec<Part>)> {
    if !is_flickable(scope) {
        return Vec::from([(0, parts(scope))]);
    }
    let mut rows: Vec<(usize, ItemRc)> = Vec::new();
    scope.visit_descendants(|item| {
        if (is_picture(item) || item.downcast::<StyledTextItem>().is_some()) && item.is_visible() {
            let (_, row_root, row) = place(item);
            if !rows.iter().any(|(index, _)| *index == row) {
                rows.push((row, row_root));
            }
        }
        core::ops::ControlFlow::<()>::Continue(())
    });
    rows.sort_by_key(|(row, _)| *row);
    rows.into_iter().map(|(row, root)| (row, parts(&root))).collect()
}

/// Selects from where the press began to `pointer`, in window coordinates.
pub(crate) fn drag_to(window_adapter: &Rc<dyn WindowAdapter>, pointer: LogicalPoint) {
    let Some((scope, unit, from, to, color)) = DRAG.with(|drag| {
        let mut drag = drag.borrow_mut();
        let drag = drag.as_mut()?;
        drag.pointer = pointer;
        Some((drag.scope.upgrade()?, drag.unit, drag.from, drag.to, drag.color))
    }) else {
        return;
    };
    let rows = rows_in(&scope);
    let Some((first_row, first_parts)) = rows.first() else { return };
    if first_parts.is_empty() {
        return;
    }
    // What's under the pointer, by the press's unit, else the end of the last part before it in
    // reading order, above it or level with it and to its left, else the start of the first.
    let mut focus = None;
    for (row, parts) in &rows {
        for (index, part) in parts.iter().enumerate() {
            if !part.rect.contains(pointer) {
                continue;
            }
            let range = if part.picture {
                Some(0..1)
            } else {
                part.item.downcast::<StyledTextItem>().and_then(|text| {
                    let local = LogicalPoint::from_lengths(
                        LogicalLength::new(pointer.x - part.rect.min_x()),
                        LogicalLength::new(pointer.y - part.rect.min_y()),
                    );
                    text.as_pin_ref().range_at(&part.item, window_adapter, local, unit)
                })
            };
            if let Some(range) = range {
                focus = Some((
                    Position { row: *row, part: index, offset: range.start },
                    Position { row: *row, part: index, offset: range.end },
                ));
            }
        }
    }
    let focus = focus.unwrap_or_else(|| {
        let before = rows.iter().flat_map(|(row, parts)| {
            parts.iter().enumerate().map(move |(index, part)| (*row, index, part))
        });
        let last = before
            .filter(|(_, _, part)| {
                part.rect.max_y() <= pointer.y
                    || (part.rect.min_y() <= pointer.y && part.rect.max_x() <= pointer.x)
            })
            .last();
        let place = match last {
            Some((row, index, part)) => Position { row, part: index, offset: part.len() },
            None => Position::row_start(*first_row),
        };
        (place, place)
    });
    let (start, end) = if focus.0 >= from { (from, focus.1.max(to)) } else { (focus.0, to) };
    let parts_of = |row: usize| rows.iter().find(|(index, _)| *index == row).map(|(_, p)| p);
    let shown = |row: usize, from: Position, to: Position| {
        parts_of(row).map(|parts| row_text(parts, row, from, to)).unwrap_or_default()
    };
    // The press's row may have scrolled away, so it's copied as it was when pressed.
    let pressed = |after: bool| {
        DRAG.with(|drag| {
            let drag = drag.borrow();
            drag.as_ref().map(|drag| if after { drag.after.clone() } else { drag.before.clone() })
        })
        .unwrap_or_default()
    };
    let (start_text, end_text) = if start.row == end.row {
        (shown(start.row, start, end), String::new())
    } else if start == from {
        (pressed(true), shown(end.row, Position::row_start(end.row), end))
    } else {
        (shown(start.row, start, Position::row_end(start.row)), pressed(false))
    };
    if start == end {
        if SELECTION.with(|selection| selection.borrow_mut().take()).is_some() {
            changed();
        }
    } else {
        set(Selection { scope: scope.downgrade(), start, end, start_text, end_text, color });
    }
    scroll_near_edge(&scope, pointer);
}

/// Scrolls the list while the pointer is near its top or bottom edge, or past it, and carries the
/// selection on with it; stops when it isn't.
fn scroll_near_edge(scope: &ItemRc, pointer: LogicalPoint) {
    const EDGE: f32 = 24.;
    let Some(flickable) = scope.downcast::<Flickable>() else { return };
    let rect = window_rect(scope);
    let above = rect.min_y() + EDGE - pointer.y;
    let below = pointer.y - (rect.max_y() - EDGE);
    let speed = if above > 0. {
        above.min(120.) / 4.
    } else if below > 0. {
        -below.min(120.) / 4.
    } else {
        0.
    };
    DRAG.with(|drag| {
        let mut drag = drag.borrow_mut();
        let Some(drag) = drag.as_mut() else { return };
        drag.speed = speed;
        if speed == 0. {
            drag.scroller.stop();
            return;
        }
        if drag.scroller.running() {
            return;
        }
        let flickable = flickable.clone();
        let scope = scope.downgrade();
        let window = drag.window.clone();
        drag.scroller.start(
            TimerMode::Repeated,
            core::time::Duration::from_millis(16),
            move || {
                let (Some(window), Some(scope)) = (window.upgrade(), scope.upgrade()) else {
                    return;
                };
                let Some((speed, pointer)) =
                    DRAG.with(|drag| drag.borrow().as_ref().map(|drag| (drag.speed, drag.pointer)))
                else {
                    return;
                };
                let flickable = flickable.as_pin_ref();
                let height = scope.geometry().height();
                let lowest = (height - flickable.content_height().get()).min(0.);
                let y = (flickable.content_y().get() + speed).clamp(lowest, 0.);
                if y != flickable.content_y().get() {
                    flickable.content_y.set(LogicalLength::new(y));
                    // As a scroll by hand would.
                    Flickable::FIELD_OFFSETS.flicked().apply_pin(flickable).call(&());
                }
                drag_to(&window, pointer);
            },
        );
    });
}

/// Ends the press, copying what it selected to the selection clipboard. Returns whether it
/// selected anything.
pub(crate) fn release(window_adapter: &Rc<dyn WindowAdapter>) -> bool {
    if let Some(drag) = DRAG.with(|drag| drag.borrow_mut().take()) {
        drag.scroller.stop();
    }
    let Some(text) = copied() else { return false };
    WindowInner::from_pub(window_adapter.window())
        .context()
        .platform()
        .set_clipboard_text(&text, Clipboard::SelectionClipboard);
    true
}

/// The selection as copied.
fn copied() -> Option<String> {
    SELECTION.with(|selection| {
        let selection = selection.borrow();
        let selection = selection.as_ref()?;
        if selection.start.row == selection.end.row {
            return Some(selection.start_text.clone());
        }
        let mut text = selection.start_text.clone();
        ROW_TEXT.with(|row_text| {
            if let Some(row_text) = row_text.borrow().as_ref() {
                for row in selection.start.row + 1..selection.end.row {
                    if let Some(row) = row_text(row).filter(|row| !row.is_empty()) {
                        text.push('\n');
                        text.push_str(&row);
                    }
                }
            }
        });
        if !selection.end_text.is_empty() {
            text.push('\n');
            text.push_str(&selection.end_text);
        }
        Some(text)
    })
}

/// Copies the selection, unless the focus is in a text input with a selection of its own, which
/// copies that instead. Returns whether it copied.
pub(crate) fn copy(window: &WindowInner) -> bool {
    let input_selecting = window.focus_item.borrow().upgrade().is_some_and(|item| {
        item.downcast::<crate::items::TextInput>()
            .is_some_and(|input| input.as_pin_ref().has_selection())
    });
    if input_selecting {
        return false;
    }
    let Some(text) = copied() else { return false };
    window.context().platform().set_clipboard_text(&text, Clipboard::DefaultClipboard);
    true
}

/// What of `item`, a text or a picture, the selection covers, and its colour. Drawing calls this,
/// so it's drawn again when the selection changes.
pub(crate) fn covered(item: &ItemRc) -> Option<(Range<usize>, Color)> {
    track();
    let (scope, start, end, color) = SELECTION.with(|selection| {
        let selection = selection.borrow();
        let selection = selection.as_ref()?;
        Some((selection.scope.upgrade()?, selection.start, selection.end, selection.color))
    })?;
    let (item_scope, row_root, row) = place(item);
    if item_scope != scope || row < start.row || row > end.row {
        return None;
    }
    let len = |item: &ItemRc| {
        if is_picture(item) {
            1
        } else {
            item.downcast::<StyledTextItem>().map_or(0, |text| text.as_pin_ref().plain().len())
        }
    };
    if row > start.row && row < end.row {
        let len = len(item);
        return (len > 0).then_some((0..len, color));
    }
    let parts = parts(&row_root);
    let part = parts.iter().position(|part| &part.item == item)?;
    Some((covered_range(parts[part].len(), row, part, start, end)?, color))
}
