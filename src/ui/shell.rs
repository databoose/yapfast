//! The window, which closing to the tray drops and opening makes again, and
//! the tray icon that keeps Talk running meanwhile, with its session, calls,
//! notifications and sounds.

use std::rc::Weak;

use super::*;
use crate::tray::{Choice, Shown, Tray};

thread_local! {
    /// Where the tray's choices go.
    static SHELL: RefCell<Weak<Shell>> = const { RefCell::new(Weak::new()) };
}

/// Talk's window while it's open, and what makes it again.
pub(super) struct Shell {
    window: RefCell<Option<TalkWindow>>,
    pub(super) adapter: Rc<RefCell<Adapter>>,
    /// Shown in every window: why Talk draws with Skia Software, if the GPU failed.
    notice: SharedString,
    gpu: bool,
    /// The renderer setting changed and Talk restarts to use it.
    restart: Cell<bool>,
    /// What the closed window showed that the application doesn't keep.
    view: RefCell<Option<View>>,
}

/// Where a closed window was, and where its timeline was scrolled to.
struct View {
    size: slint::PhysicalSize,
    position: slint::PhysicalPosition,
    maximized: bool,
    /// The message at the top of the timeline, unless the newest showed.
    top: Option<SharedString>,
}

impl Shell {
    /// Takes the first window, bound and refreshed, and puts Talk in the tray.
    pub(super) fn new(
        window: TalkWindow,
        adapter: Rc<RefCell<Adapter>>,
        notice: SharedString,
        gpu: bool,
    ) -> Rc<Self> {
        let shell = Rc::new(Self {
            window: RefCell::new(None),
            adapter,
            notice,
            gpu,
            restart: Cell::new(false),
            view: RefCell::new(None),
        });
        shell.watch(&window);
        *shell.window.borrow_mut() = Some(window);
        SHELL.with(|current| *current.borrow_mut() = Rc::downgrade(&shell));
        OPEN.with(|open| {
            *open.borrow_mut() = Some(Box::new(|room| {
                if let Some(shell) = SHELL.with(|shell| shell.borrow().upgrade()) {
                    shell.open_room(room);
                }
            }));
        });
        let tray = Tray::start(chosen);
        let mut adapter = shell.adapter.borrow_mut();
        adapter.tray = tray;
        adapter.show_tray();
        drop(adapter);
        shell
    }

    /// Whether Talk ends to restart with another renderer.
    pub(super) fn restarting(&self) -> bool {
        self.restart.get()
    }

    pub(super) fn window(&self) -> Option<TalkWindow> {
        self.window
            .borrow()
            .as_ref()
            .map(ComponentHandle::clone_strong)
    }

    /// What every window does beyond its callbacks: closes to the tray, and
    /// restarts Talk.
    fn watch(self: &Rc<Self>, window: &TalkWindow) {
        let shell = Rc::downgrade(self);
        window.window().on_close_requested(move || {
            let shell = shell.clone();
            // Not from inside the window's own handler, which drops it.
            slint::Timer::single_shot(Duration::ZERO, move || {
                if let Some(shell) = shell.upgrade() {
                    shell.close();
                }
            });
            slint::CloseRequestResponse::KeepWindowShown
        });
        let shell = Rc::downgrade(self);
        window.on_restart(move || {
            if let Some(shell) = shell.upgrade() {
                shell.restart.set(true);
            }
            let _ = slint::quit_event_loop();
        });
    }

    /// The window's close button: Talk goes on in the tray, or without a tray
    /// to show it, or with the setting off, quits.
    pub(super) fn close(&self) {
        let to_tray = {
            let adapter = self.adapter.borrow();
            adapter.layout.close_to_tray.unwrap_or(true)
                && adapter.tray.as_ref().is_some_and(Tray::available)
        };
        if !to_tray {
            tracing::info!("window closed without a tray to close to; quitting");
            let _ = slint::quit_event_loop();
            return;
        }
        self.drop_window();
        // Slint lets go of the window's last pieces as the event loop goes on.
        slint::Timer::single_shot(Duration::from_millis(500), release_memory);
    }

    /// Drops the window, its component, models' views and renderer, keeping
    /// where it was for the next.
    pub(super) fn drop_window(&self) {
        let Some(window) = self.window.borrow_mut().take() else {
            return;
        };
        let native = window.window();
        let top = window.get_top_item();
        *self.view.borrow_mut() = Some(View {
            size: native.size(),
            position: native.position(),
            maximized: native.is_maximized(),
            top: (!window.invoke_timeline_at_end() && !top.is_empty()).then_some(top),
        });
        self.adapter.borrow_mut().detach();
        // A shown window keeps itself alive; hide it, then drop the last handle.
        let weak = window.as_weak();
        if let Err(error) = window.hide() {
            tracing::warn!(%error, "could not hide the window");
        }
        drop(window);
        FOCUSED.set(false);
        tracing::info!(released = weak.upgrade().is_none(), "window closed");
    }

    /// Brings the window to the front, making it again if it was closed.
    pub(super) fn open(self: &Rc<Self>) -> Option<TalkWindow> {
        if let Some(window) = self.window() {
            let _ = window.show();
            window.window().set_minimized(false);
            window
                .window()
                .with_winit_window(|winit| winit.focus_window());
            return Some(window);
        }
        self.make()
            .inspect_err(|error| tracing::error!(%error, "could not open the window"))
            .ok()
    }

    /// Opens the window on a conversation, as a clicked notification does.
    fn open_room(self: &Rc<Self>, room: String) {
        if let Some(window) = self.open() {
            window.set_settings_open(false);
            self.adapter
                .borrow_mut()
                .action(&window, Action::Open(room));
        }
    }

    /// Makes the window again, as the closed one was: the same size and place,
    /// the conversation scrolled to the same message, and everything else from
    /// the application's state.
    fn make(self: &Rc<Self>) -> Result<TalkWindow, slint::PlatformError> {
        let window = TalkWindow::new()?;
        let view = self.view.borrow_mut().take();
        if let Some(view) = &view {
            let native = window.window();
            if view.maximized {
                native.set_maximized(true);
            } else {
                native.set_size(view.size);
                native.set_position(view.position);
            }
        }
        window.show()?;
        prepare(&window);
        window.set_graphics_notice(self.notice.clone());
        window.set_gpu_running(self.gpu);
        self.adapter.borrow_mut().attach(&window);
        bind_callbacks(&window, &self.adapter);
        watch_window(&window, &self.adapter);
        self.watch(&window);
        self.adapter.borrow_mut().refresh(&window);
        let adapter = self.adapter.borrow();
        let state = &adapter.app.state;
        if let Some(top) = view.and_then(|view| view.top)
            && let Some(index) = state
                .messages
                .iter()
                .position(|message| message.item == top.as_str())
        {
            show_message(&window, index, &top, false);
        }
        if state.selected_room().is_some_and(|room| !room.voice) {
            window.invoke_focus_composer();
        }
        drop(adapter);
        *self.window.borrow_mut() = Some(window.clone_strong());
        tracing::info!("window opened again");
        Ok(window)
    }

    /// Does what a button in the window would, with the window or without it.
    fn act(&self, action: Action) {
        let mut adapter = self.adapter.borrow_mut();
        match self.window() {
            Some(window) => adapter.action(&window, action),
            None => {
                let effects = adapter.app.dispatch(action);
                adapter.execute(effects);
                adapter.show_tray();
            }
        }
    }
}

/// What the person chose in the tray.
fn chosen(choice: Choice) {
    let Some(shell) = SHELL.with(|shell| shell.borrow().upgrade()) else {
        return;
    };
    match choice {
        Choice::Open => {
            shell.open();
        }
        Choice::Disconnect => shell.act(Action::Voice(voice::Command::Leave)),
        Choice::Quit => {
            let _ = slint::quit_event_loop();
        }
        Choice::Available => {
            if let Some(window) = shell.window() {
                let adapter = shell.adapter.borrow();
                window.set_tray_available(adapter.tray.as_ref().is_some_and(Tray::available));
            }
        }
    }
}

impl Adapter {
    /// The window closed: what only it used stops with it. The models stay, so
    /// the next window shows them without building them again.
    pub(super) fn detach(&mut self) {
        self.meter = None;
        #[cfg(feature = "discord")]
        {
            self.gate_meter = None;
        }
        self.camera_test = None;
        self.viewer_image = None;
        if let Some(dialog) = self.dialog.take() {
            dialog.abort();
        }
        self.window = slint::Weak::default();
    }

    /// Rings for a DM call that rings this account, except in Do Not Disturb,
    /// which shows the call without ringing.
    pub(super) fn ring(&mut self) {
        #[cfg(feature = "discord")]
        {
            let state = &self.app.state;
            if ringing(state).next().is_none() || state.do_not_disturb() {
                self.ringer = None;
            } else if self.ringer.is_none() {
                self.ringer = talk_core::ring::Ringer::start(
                    talk_core::ring::Tone::Incoming,
                    state.voice.speaker.as_deref(),
                )
                .inspect_err(|error| tracing::warn!(%error, "could not ring"))
                .ok();
            }
        }
    }

    /// The tray icon's unread dot, for a mention or an unread DM as Discord
    /// counts them, and Disconnect in its menu during a call.
    pub(super) fn show_tray(&self) {
        let Some(tray) = &self.tray else { return };
        let state = &self.app.state;
        tray.show(Shown {
            unread: state
                .rooms
                .iter()
                .any(|room| room.mentions > 0 || (room.direct && !room.muted && room.unread > 0)),
            call: state.voice.active(),
        });
    }
}

/// Gives back memory freed with the window. On Linux glibc keeps freed heap
/// for reuse until trimmed. On Windows the heap is compacted, which releases
/// its free blocks, and the working set emptied: Task Manager shows the
/// private working set, which pages the window used would otherwise stay in.
pub(super) fn release_memory() {
    trim();
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::{
            Memory::{GetProcessHeap, HeapCompact},
            Threading::{GetCurrentProcess, SetProcessWorkingSetSize},
        };
        // SAFETY: both act on this process's own heap and working set; -1 for
        // both sizes asks Windows to take out every page it can.
        unsafe {
            HeapCompact(GetProcessHeap(), 0);
            SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
        }
    }
    tracing::info!("memory released");
}
