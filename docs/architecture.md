# Application core and client UI

Talk's Cargo workspace has four packages, all in one process. The root `talk`
package is the executable: the Slint adapter and the command-line entry point.
[`crates/talk-core`](../crates/talk-core) holds application behavior and the
native Matrix, Discord and voice backend.
[`crates/talk-discord`](../crates/talk-discord) holds the Discord protocol:
login, the gateway, the HTTP API and voice. [`crates/talk-ui`](../crates/talk-ui)
compiles `ui/talk.slint` and the files it imports in its `build.rs`, so editing
any `.slint` file rebuilds only that crate.

```mermaid
flowchart LR
    UI[Slint widgets and Skia rendering] -->|Actions| App[Application model]
    App -->|State| UI
    App -->|Effects| Adapter[Client event loop]
    Adapter -->|Commands| Backend[Native Matrix and voice backend]
    Backend -->|Events| Adapter
    Adapter -->|Apply events| App
```

## Shared application behavior

[`application.rs`](../crates/talk-core/src/application.rs) owns navigation,
remembered channels, per-room drafts, reply and edit modes, delete confirmation,
selected timelines, account and trust status, history loading, call controls and
voice-switch confirmation. The backend saves drafts in the encrypted session
store and sends through the SDK's persistent send queue, which owns retries.
Its local preview uses the same model and emits no network effects.

A client sends an `Action` to `Application::dispatch` and applies backend events
with `Application::apply`. These methods return effects for starting a session
or sending a backend command. `AppState` supplies ordinary Rust data for rendering,
including shared timeline storage through `Arc<Vec<ChatMessage>>`. `dispatch`
routes each action to a method in the module for its area, under
[`application/`](../crates/talk-core/src/application): `compose`, `messages`,
`rooms`, `profiles`, `calls`, `session`, `preview` and `discord_account`.

The Slint adapter in [`src/ui.rs`](../src/ui.rs) and [`src/ui/`](../src/ui), with
the interface in [`ui/*.slint`](../ui), owns input widgets, password masking,
focus, filtering, scroll position, list virtualization, device selectors, window
state, theme and icons. `Adapter::refresh`, in
[`src/ui/refresh.rs`](../src/ui/refresh.rs), shows the application's state one
area at a time: the image viewer, profile, workspace, sidebar, account, header,
member lists, dialogs, composer, messages and call. `ui/talk.slint` holds the
window and imports its parts from the files beside it: the theme, controls, rail,
composer, messages, people, friends, voice, dialogs, settings, sign-in and
Discord pieces. The adapter updates widgets after model changes. A programmatic
composer update uses the new room's draft, so it cannot overwrite that draft with
the previous room's text.

## How a UI change flows through the code

A visible change usually touches each of these steps:

1. **Slint.** A component in `ui/<area>.slint` declares a property or callback.
   Rust sees only `TalkWindow` in `ui/talk.slint` and the structs it exports, so
   `TalkWindow` declares a matching property or callback and binds it to the
   component. The areas are `theme`, `controls`, `rail`, `composer`, `messages`,
   `people`, `friends`, `voice`, `dialogs`, `settings`, `sign-in` and `discord`.
2. **State to window.** `Adapter::refresh` calls one `show_<area>` method in
   `src/ui/refresh.rs` per area (`show_viewer`, `show_profile`,
   `show_workspace`, `show_sidebar`, `show_account`, `show_header`,
   `show_members`, `show_dialogs`, `show_composer`, `show_messages`,
   `show_voice`), which sets the window's properties from `AppState`. Message
   rows are built in `src/ui/messages.rs`, profile rows in `src/ui/profile.rs`
   and pictures in `src/ui/pictures.rs`.
3. **Input.** `bind_callbacks` in `src/ui/callbacks.rs` registers an `on_*`
   handler for each window callback. Most turn the input into an `Action` and
   pass it to `Adapter::action` in `src/ui.rs`.
4. **Application.** `Adapter::action` calls `Application::dispatch`, executes
   the returned effects, then refreshes the window. A composer keystroke
   (`SetDraft` or `MentionQuery`) refreshes only the composer. A backend event
   shows only what it can change: who's typing shows only the composer, and
   the call, with who's speaking, only the call's views (`Reach` in
   `src/ui/refresh.rs`). The sidebar is built again only once what it shows
   changed. `Action` and
   `dispatch` are in `crates/talk-core/src/application.rs`, which routes each
   action to the module for its area in `crates/talk-core/src/application/`:
   `compose.rs`, `messages.rs`, `rooms.rs`, `profiles.rs`, `calls.rs`,
   `session.rs`, `preview.rs` or `discord_account.rs`.
5. **Backend.** An `Effect::Backend(Command)` goes through `Backend::send` to
   the chat actor in `crates/talk-core/src/chat.rs`, which routes it to the
   network or to voice. Matrix lives
   in `matrix_chat.rs` and `matrix_voice.rs`; Discord in `discord.rs`,
   `discord_text.rs`, `discord_voice.rs`, `discord_audio.rs` and
   `discord_link.rs`, all under `crates/talk-core/src/`. The Discord protocol
   itself is in `crates/talk-discord`, whose gateway session is
   `crates/talk-discord/src/service.rs`. Backend events come back through
   `Application::apply`.

## Native backend ownership

[`backend.rs`](../crates/talk-core/src/backend.rs) starts the chat actor and owns
its command sender, background task and runtime reference. The client retains
this handle for the session, independently of the selected text or voice channel.
Incoming events arrive through a Tokio receiver, so each UI framework can choose
how to wake its event loop.

Slint waits asynchronously on that receiver and updates presentation on its UI
thread. No timer polls the network or redraws an idle window. The adapter retains
one backend across navigation and cancels its event receiver before releasing
the backend. Widget models update changed rows instead of replacing every list
on voice or sync events.

The actor owns one Matrix session and its sync service. The voice controller
leases that same client and encrypted store. Encryption and device trust checks
remain authoritative in the backend. Explicit logout stops voice and sync before
revoking the session; failed logout retains the account. Dropping the backend on
the client thread cancels its work without revoking saved credentials.

The core has no GPUI or other UI-framework dependency. Its native services still
depend on Tokio, SQLite, OS credential storage and libwebrtc. A browser client
will need platform implementations for those services. This split shares app
behavior without claiming the native backend already runs on the web or Android.

## Adding another native UI

1. Depend on `talk-core` and render its `AppState`.
2. Dispatch user actions and execute returned effects. For a start effect, retain
   the `Backend` handle and receive its events. For a command effect, call `send`.
3. Feed rejected commands to `Application::backend_failed` and closed event streams
   to `Application::stopped`, then render the updated state.
4. Keep widget-specific work in the client. `selection_version` identifies a room
   change or re-selection; `reset_version` identifies a return to sign-in. Compare
   timeline `Arc` identities to detect timeline replacement without copying it.

Framework experiments can use the shared preview first and connect to the same
native backend afterward. They must preserve the model's behavior rather than
implement their own drafts, send handling or voice lifecycle.

## Checks and packaging

`cargo run --locked` and `--preview` keep their existing behavior. The headless
variant uses `--no-default-features --features matrix,discord`. `scripts/check`
checks the whole workspace; see [building](building.md#checks). Ignored chat and
voice tests now belong to `talk-core`, so select them with
`cargo test -p talk-core`.

The Windows source archive includes `crates/` and `ui/`. Existing session storage paths and
formats are unchanged. Library tests keep disposable fixtures under the workspace's
ignored `.local` directory even though Cargo runs them from the crate directory.

Validation on 2026-10-02 passed formatting, workspace Clippy with warnings denied,
15 unit tests, a desktop build, and desktop/headless smoke checks. The local
Matrix integration test passed encrypted delivery, session restoration, safe
retries, trust restrictions and channel updates. `scripts/test-voice --fast`
passed native encrypted audio, mute/deafen, late joining, departure and stalled
join cancellation through isolated virtual audio devices. A dependency-tree
check found no UI-framework dependencies in `talk-core`.

The Windows x64 MSVC compilation check passed for both workspace packages and
all targets using `cargo xwin check` with `--locked --workspace --all-targets`
and target `x86_64-pc-windows-msvc`. This checks compilation; the refactored
executable has not yet had a Windows runtime test.

The GPUI preview opened and sent a local message with Enter. Future GUI trials
must use an isolated display as described in the comparison plan.

The 2026-10-03 [Slint validation](slint-client.md) passed the complete Linux check
script, 3 desktop tests in addition to the 15 core tests, and a separate encrypted
chat integration test. Private GPU/software UI fixtures passed real encrypted
sending, close, session restoration and logout. Current Windows evidence and
remaining manual checks are recorded in the [Windows guide](windows-beta.md).
