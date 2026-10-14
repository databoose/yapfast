//! Keeps the window moving while Windows drags or resizes it by its frame.
//!
//! Windows runs a message loop of its own from the press on the title bar or
//! border until the release. winit still passes window messages on, but its
//! own loop doesn't run, so Slint's timers and animation clock, which advance
//! when it does, stand still: animations stop, and work deferred to a timer,
//! such as pictures that arrived, waits for the release. While that loop
//! runs, a Win32 timer, which Windows calls even from its own loop, advances
//! them instead, as winit's loop would, and redraws what animates. The timer
//! starts with the first move or resize winit reports and stops once Windows
//! isn't moving or sizing the window.

use std::cell::{Cell, RefCell};

use slint::ComponentHandle;

use windows_sys::Win32::{
    Foundation::HWND,
    System::Threading::GetCurrentThreadId,
    UI::WindowsAndMessaging::{
        GUI_INMOVESIZE, GUITHREADINFO, GetGUIThreadInfo, KillTimer, SetTimer,
    },
};

use super::TalkWindow;

/// How often the timer runs: about a frame at 60 Hz.
const TICK_MS: u32 = 16;

thread_local! {
    /// The window to redraw, and the running timer's id, or 0.
    static WINDOW: RefCell<slint::Weak<TalkWindow>> = RefCell::new(slint::Weak::default());
    static TIMER: Cell<usize> = const { Cell::new(0) };
}

/// The window moved or resized: if Windows is dragging it, keep it moving
/// until it stops.
pub(super) fn moved(window: &slint::Weak<TalkWindow>) {
    if TIMER.get() != 0 || !in_move_size() {
        return;
    }
    WINDOW.with_borrow_mut(|shown| *shown = window.clone());
    // SAFETY: a thread timer, with no window; Windows calls `tick` on this
    // thread from whichever message loop is running.
    let id = unsafe { SetTimer(std::ptr::null_mut(), 0, TICK_MS, Some(tick)) };
    tracing::debug!(id, "the window is being moved or sized; keeping it moving");
    TIMER.set(id);
}

unsafe extern "system" fn tick(_: HWND, _: u32, id: usize, _: u32) {
    if !in_move_size() {
        // SAFETY: the thread timer `moved` started.
        unsafe { KillTimer(std::ptr::null_mut(), id) };
        tracing::debug!(id, "the window stopped moving or sizing");
        TIMER.set(0);
        return;
    }
    slint::platform::update_timers_and_animations();
    let animating = i_slint_core::animations::CURRENT_ANIMATION_DRIVER
        .with(i_slint_core::animations::AnimationDriver::has_active_animations);
    if animating && let Some(window) = WINDOW.with_borrow(slint::Weak::upgrade) {
        window.window().request_redraw();
    }
}

/// Whether Windows is moving or sizing a window of this thread.
fn in_move_size() -> bool {
    let mut info = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is a GUITHREADINFO with its size set, as the call asks.
    let known = unsafe { GetGUIThreadInfo(GetCurrentThreadId(), &mut info) } != 0;
    known && info.flags & GUI_INMOVESIZE != 0
}
