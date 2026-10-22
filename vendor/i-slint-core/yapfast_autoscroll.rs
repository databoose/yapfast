// Yapfast: not part of upstream Slint. See docs/building.md in Yapfast.

//! Autoscroll: a middle click in something that scrolls scrolls it towards the pointer, faster
//! the farther the pointer is from where it clicked, as browsers do.
//!
//! A middle press offers each flickable it passes through that has more to show than fits, and
//! the innermost one scrolls; a text input it lands in takes it instead, to paste. Clicking
//! starts a scroll that lasts until the next press, a key or a wheel, and holding the button
//! and moving away scrolls until it's let go. While it lasts the window gives its pointer
//! events to the scroll rather than to the items under the pointer, and the press that ends it
//! goes no further either.

use alloc::rc::{Rc, Weak};
use core::cell::RefCell;
use core::pin::Pin;

use crate::Property;
use crate::animations::Instant;
use crate::input::MouseEvent;
use crate::item_tree::{ItemRc, ItemWeak};
use crate::items::{BuiltInMouseCursor, Flickable, PointerEventButton};
use crate::lengths::{LogicalLength, LogicalPoint, LogicalVector};
use crate::timers::{Timer, TimerMode};
use crate::window::{WindowAdapter, WindowInner};

/// How far the pointer can be from where it clicked, in logical pixels, without scrolling.
const STILL: f32 = 10.;
/// How fast the pointer scrolls just past `STILL`, in logical pixels a second for each pixel.
const SPEED: f32 = 8.;
/// How much faster it scrolls the farther it is: pixels a second for each pixel squared.
const ACCELERATION: f32 = 0.05;
/// The fastest it scrolls, in logical pixels a second.
const FASTEST: f32 = 12000.;

/// Where an autoscroll began, for the application to show it: in window coordinates, with the
/// directions it can go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Marker {
    pub position: LogicalPoint,
    pub vertical: bool,
    pub horizontal: bool,
}

struct Scroll {
    flickable: ItemWeak,
    window: Weak<dyn WindowAdapter>,
    /// Where the press was, and where the pointer is, in window coordinates.
    origin: LogicalPoint,
    pointer: LogicalPoint,
    /// While the middle button that started it is down; moving away with it down makes letting
    /// go end the scroll.
    held: bool,
    dragged: bool,
    /// The button of the press that ended the scroll, until it's let go.
    ending: Option<PointerEventButton>,
    last_tick: Instant,
    timer: Timer,
}

std::thread_local! {
    /// The innermost flickable the middle press being dispatched passed through.
    static OFFERED: RefCell<Option<ItemWeak>> = const { RefCell::new(None) };
    static SCROLL: RefCell<Option<Scroll>> = const { RefCell::new(None) };
    static LISTENER: RefCell<Option<Rc<dyn Fn(&crate::api::Window, Option<Marker>)>>> =
        const { RefCell::new(None) };
}

/// Calls `listener` as each autoscroll starts and ends, to show where it began.
pub fn on_marker(listener: impl Fn(&crate::api::Window, Option<Marker>) + 'static) {
    LISTENER.with(|cell| *cell.borrow_mut() = Some(Rc::new(listener)));
}

fn tell(window: &Rc<dyn WindowAdapter>, marker: Option<Marker>) {
    if let Some(listener) = LISTENER.with(|cell| cell.borrow().clone()) {
        listener(window.window(), marker);
    }
}

/// The directions `flickable` can scroll in: vertically, and horizontally.
fn directions(flickable: &ItemRc) -> (bool, bool) {
    let Some(item) = flickable.downcast::<Flickable>() else {
        return (false, false);
    };
    let item = item.as_pin_ref();
    let size = flickable.geometry().size;
    (
        item.content_height().get() > size.height + 0.5,
        item.content_width().get() > size.width + 0.5,
    )
}

/// A middle press passed through `flickable` on its way to the items under it.
pub(crate) fn offer(flickable: &ItemRc) {
    let can = directions(flickable);
    if can.0 || can.1 {
        OFFERED.with(|offered| *offered.borrow_mut() = Some(flickable.downgrade()));
    }
}

/// A text input took the middle press, to paste.
pub(crate) fn decline() {
    OFFERED.with(|offered| offered.borrow_mut().take());
}

/// Whether an autoscroll is going on, or ending.
pub(crate) fn active() -> bool {
    SCROLL.with(|scroll| scroll.borrow().is_some())
}

/// Before a middle press is dispatched.
pub(crate) fn pressing() {
    OFFERED.with(|offered| offered.borrow_mut().take());
}

/// After a middle press at `position`, in window coordinates, was dispatched: starts scrolling
/// the flickable it offered, if any. Returns the pointer to show if it did, for the items the
/// press reached to let go of it.
pub(crate) fn start(
    window: &Rc<dyn WindowAdapter>,
    position: LogicalPoint,
) -> Option<BuiltInMouseCursor> {
    let flickable = OFFERED
        .with(|offered| offered.borrow_mut().take())?
        .upgrade()?;
    let (vertical, horizontal) = directions(&flickable);
    let timer = Timer::default();
    let flickable_weak = flickable.downgrade();
    let window_weak = Rc::downgrade(window);
    timer.start(
        TimerMode::Repeated,
        core::time::Duration::from_millis(16),
        move || {
            tick(&flickable_weak, &window_weak);
        },
    );
    let context = WindowInner::from_pub(window.window()).context();
    let scroll = Scroll {
        flickable: flickable.downgrade(),
        window: Rc::downgrade(window),
        origin: position,
        pointer: position,
        held: true,
        dragged: false,
        ending: None,
        last_tick: Instant::now(&context),
        timer,
    };
    let shown = cursor(&scroll);
    SCROLL.with(|cell| *cell.borrow_mut() = Some(scroll));
    tell(
        window,
        Some(Marker {
            position,
            vertical,
            horizontal,
        }),
    );
    Some(shown)
}

/// How fast the pointer `distance` from where it clicked scrolls, in pixels a second.
fn speed(distance: f32) -> f32 {
    let past = distance.abs() - STILL;
    if past <= 0. {
        return 0.;
    }
    (past * SPEED + past * past * ACCELERATION)
        .min(FASTEST)
        .copysign(distance)
}

fn tick(flickable: &ItemWeak, window: &Weak<dyn WindowAdapter>) {
    let (Some(flickable_rc), Some(window)) = (flickable.upgrade(), window.upgrade()) else {
        stop();
        return;
    };
    let Some(item) = flickable_rc.downcast::<Flickable>() else {
        return;
    };
    let now = Instant::now(&WindowInner::from_pub(window.window()).context());
    let Some((distance, elapsed)) = SCROLL.with(|scroll| {
        let mut scroll = scroll.borrow_mut();
        let scroll = scroll.as_mut()?;
        let elapsed = (now - scroll.last_tick).as_secs_f32().min(0.05);
        scroll.last_tick = now;
        Some((scroll.pointer - scroll.origin, elapsed))
    }) else {
        return;
    };
    let item = item.as_pin_ref();
    let size = flickable_rc.geometry().size;
    let mut moved = false;
    let mut scroll_by =
        |content: Pin<&Property<LogicalLength>>, visible: f32, length: f32, distance: f32| {
            let at = content.get().get();
            let next = (at - speed(distance) * elapsed).clamp((visible - length).min(0.), 0.);
            if next != at {
                content.set(LogicalLength::new(next));
                moved = true;
            }
        };
    scroll_by(
        Flickable::FIELD_OFFSETS.content_y().apply_pin(item),
        size.height,
        item.content_height().get(),
        distance.y,
    );
    scroll_by(
        Flickable::FIELD_OFFSETS.content_x().apply_pin(item),
        size.width,
        item.content_width().get(),
        distance.x,
    );
    if moved {
        // As a scroll by hand would.
        Flickable::FIELD_OFFSETS.flicked().apply_pin(item).call(&());
    }
}

/// Ends an autoscroll, as a key or the window going inactive does. Returns whether one was going
/// on, for the pointer to look as usual again.
pub(crate) fn stop() -> bool {
    let Some(scroll) = SCROLL.with(|scroll| scroll.borrow_mut().take()) else {
        return false;
    };
    scroll.timer.stop();
    if scroll.ending.is_none()
        && let Some(window) = scroll.window.upgrade()
    {
        tell(&window, None);
    }
    true
}

/// What the window does with a pointer event while an autoscroll is going on.
pub(crate) enum Handling {
    /// It dispatches it as usual, with the pointer as it was.
    Dispatch,
    /// It drops it, and shows the pointer as this.
    Take(BuiltInMouseCursor),
    /// The autoscroll is over: it dispatches it as usual, with the pointer as usual.
    Ended,
}

/// Before the window dispatches `event`.
pub(crate) fn handle(event: &MouseEvent) -> Handling {
    if !active() {
        return Handling::Dispatch;
    }
    // The press that ends a scroll, until it's let go.
    let ending = SCROLL.with(|scroll| scroll.borrow().as_ref().and_then(|scroll| scroll.ending));
    if let Some(button) = ending {
        return match event {
            MouseEvent::Released {
                button: released, ..
            } if *released == button => {
                stop();
                Handling::Take(BuiltInMouseCursor::Default)
            }
            MouseEvent::Pressed { .. } | MouseEvent::Released { .. } | MouseEvent::Moved { .. } => {
                Handling::Take(BuiltInMouseCursor::Default)
            }
            MouseEvent::Exit => Handling::Dispatch,
            _ => {
                stop();
                Handling::Ended
            }
        };
    }
    match event {
        MouseEvent::Moved { position, .. } => SCROLL.with(|scroll| {
            let mut scroll = scroll.borrow_mut();
            let scroll = scroll.as_mut().unwrap();
            scroll.pointer = *position;
            if scroll.held && (scroll.pointer - scroll.origin).length() > STILL {
                scroll.dragged = true;
            }
            Handling::Take(cursor(scroll))
        }),
        MouseEvent::Pressed { button, .. } => {
            let window = SCROLL.with(|scroll| {
                let mut scroll = scroll.borrow_mut();
                let scroll = scroll.as_mut().unwrap();
                scroll.timer.stop();
                scroll.ending = Some(*button);
                scroll.window.upgrade()
            });
            if let Some(window) = window {
                tell(&window, None);
            }
            Handling::Take(BuiltInMouseCursor::Default)
        }
        MouseEvent::Released {
            button: PointerEventButton::Middle,
            ..
        } => {
            let (dragged, shown) = SCROLL.with(|scroll| {
                let mut scroll = scroll.borrow_mut();
                let scroll = scroll.as_mut().unwrap();
                scroll.held = false;
                (scroll.dragged, cursor(scroll))
            });
            if dragged {
                stop();
                Handling::Take(BuiltInMouseCursor::Default)
            } else {
                Handling::Take(shown)
            }
        }
        MouseEvent::Released { .. } => {
            SCROLL.with(|scroll| Handling::Take(cursor(scroll.borrow().as_ref().unwrap())))
        }
        MouseEvent::Exit => Handling::Dispatch,
        _ => {
            stop();
            Handling::Ended
        }
    }
}

/// The pointer for `scroll`: arrows the ways it can go, or the way it's going.
fn cursor(scroll: &Scroll) -> BuiltInMouseCursor {
    let (vertical, horizontal) = scroll
        .flickable
        .upgrade()
        .map_or((false, false), |flickable| directions(&flickable));
    let distance: LogicalVector = scroll.pointer - scroll.origin;
    let up = vertical && distance.y < -STILL;
    let down = vertical && distance.y > STILL;
    let left = horizontal && distance.x < -STILL;
    let right = horizontal && distance.x > STILL;
    use BuiltInMouseCursor::*;
    match (up, down, left, right) {
        (true, _, true, _) => NwResize,
        (true, _, _, true) => NeResize,
        (_, true, true, _) => SwResize,
        (_, true, _, true) => SeResize,
        (true, ..) => NResize,
        (_, true, ..) => SResize,
        (.., true, _) => WResize,
        (.., true) => EResize,
        _ if vertical && horizontal => Move,
        _ if horizontal => EwResize,
        _ => NsResize,
    }
}
