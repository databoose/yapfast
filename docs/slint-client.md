# Slint desktop client

On 2026-10-03, Talk's production desktop UI moved from GPUI to Slint 1.18.1.
The Matrix/session/voice services and application model stay in `talk-core`.
The existing backend dependency pins, crypto stores and account directory format
remain unchanged. `archive/gpui-before-slint` preserves the complete pre-migration
state at `4bbc6ad`; the earlier main snapshot also has an archive branch.
No remote push is part of this migration.

## Delivered behavior

- Password sign-in, saved-session restoration, recovery-key entry, trust refresh,
  sync retry and sign-out. Passwords and recovery keys are masked and cleared
  after starting the corresponding action or leaving the session.
- DMs at the top of the server rail, joined Spaces below, and only the chosen
  server's channels in the sidebar. Filtering, per-room drafts and each server's
  remembered channel use the shared application model.
- A selectable text timeline, earlier-message loading, encrypted sending and
  preserved send retries. New messages follow the bottom when already there;
  loading earlier history retains the reading position. Unencrypted rooms remain
  readable; the core continues to require a trusted device for sending.
- Persistent voice view and controls: join muted, wait alone, participants,
  leave, mute/deafen, native device selection and explicit channel-switch
  confirmation. Browsing text, DMs or another server retains the backend session
  and voice connection. Preview voice never opens an audio device.
- A Discord-style layout set in Satoshi (see [Chat UI](chat-ui.md#layout-and-look)),
  Lucide SVG icons, custom text fields, and window sizing from 1120×760 down to
  800×560. Settings, including audio devices, open in a popover above the account
  panel, dismissed by its close button, Escape or clicking outside.

`ui/talk.slint` defines the controls and layout; `src/ui.rs` adapts application
state and effects. The `talk-ui` crate compiles the interface. Its generated code
is large, so it lives apart from the application: changing Talk's Rust code does
not recompile it. Backend events wake Slint's executor through the Tokio
receiver. There is no idle polling timer. Stable widget models update changed
rows, and shutdown cancels event reception before releasing the backend actor.

## Renderers and native builds

Skia GPU is the default. Start the local preview from the repository folder:

```sh
nix develop path:./nix --command cargo run --locked -- --preview
```

For Skia Software:

```sh
nix develop path:./nix --command cargo run --locked -- --preview --renderer software
```

Omit `--preview` for normal sign-in or session restoration. The current app uses
stock Slint renderer code, without benchmark timing hooks.

### Automatic renderer recovery

Without `--renderer`, Talk starts Skia GPU through an explicit `skia-opengl`
request. The generic `skia` renderer would silently substitute a software surface
when OpenGL fails, so Talk owns the fallback instead:

- Before the GPU start, Talk writes `graphics.json` in the data folder with state
  `starting`. Three seconds after the event loop starts, it checks that the native
  window exists and removes the record. A normal close also confirms the start.
- If window creation, the GPU surface or the event loop fails before that, Talk
  stores the technical reason as `failed`, releases the session and runtime, and
  restarts itself with `--renderer software`. Linux replaces the process; Windows
  starts the new process and returns its exit code. The window shows a dismissible
  notice; the log and record keep the technical reason.
- A `starting` record found at launch means the previous GPU start never got
  confirmed, for example because a driver crashed or hung. That launch uses Skia
  Software and records the failure.
- Later automatic launches use Skia Software while a `failed` record from the same
  Talk version exists. A new version, or a confirmed `--renderer gpu` start, retries
  the GPU. `--renderer gpu` and `--renderer software` never fall back.
- A GPU failure after confirmation, such as a lost device, also restarts with Skia
  Software but is not remembered for later launches.

Slint 1.18 waits up to 500 ms for the desktop appearance query before creating
the first window. A GPU surface failure at that point is stored without stopping
the event loop, which leaves a running process with no window. The confirmation
therefore checks for the native window through Slint's version-pinned
`unstable-winit-030` accessor rather than relying on the event loop error. A
rendering notifier would also work, but Skia disables partial repainting while
one is installed.

`testing/probe_renderer_fallback.py` checks this policy on the private compositor.
It hides every EGL vendor from libglvnd to simulate a missing OpenGL driver:

```sh
nix-shell --impure experiments/frameworks/shell.nix --run 'scripts/run-framework-isolated --no-services --compositor-log .local/fallback.weston.log -- python3 testing/probe_renderer_fallback.py --output .impeccable/review/renderer-fallback'
```

On 2026-10-03 all seven checks passed on the NVIDIA host: a broken EGL start
restarted into Skia Software with the notice and recorded glutin's error; the
next automatic start used software directly; `--renderer gpu` with broken EGL
exited with an error and no fallback; a confirmed `--renderer gpu` start cleared
the record; automatic mode then confirmed Skia GPU; and a process killed 0.8 s
into its GPU start made the next launch use software.

The Nix shells supply checksum-pinned native inputs. Linux uses the upstream
Skia 0.153.3 archive. Windows builds the same pinned Skia source and dependencies
with `/MT`, matching WebRTC's static release runtime. The upstream Windows
archive uses `/MD` and cannot link with this media stack. The Windows source
builder uses two native jobs, caches its result, and verifies the runtime
directives before accepting the archive. Both libraries define `jpeg_natural_order`, so the Skia archive
preparation privately renames that one table and its references. It verifies
symbols and every ELF/COFF section's code/data and relocations before deterministic
repacking. WebRTC codec code remains intact. Production Cargo disables implicit
Skia source compilation; the separate Windows builder uses the pinned Nix inputs.

The Windows package includes the Slint UI build sources and Skia notices.
`run-talk.cmd --renderer software` selects the CPU mode. GPUI's FXC shader
cross-build is no longer used.

## Validation and limits

The automated GUI harness runs only on a private headless Weston compositor,
with a virtual input seat, private D-Bus and private XDG directories. Connected
fixtures disable D-Bus service activation, exercise the existing Linux private-file
credential fallback and use disposable local accounts. They open no audio devices
and never access the user's cursor, focus or desktop session.

The capture script is `testing/probe_slint_ui.py`. Preview captures exercise local
sending, scoped navigation, draft restoration, voice persistence, mute/deafen,
the device popover, sign-out and password masking. Coordinate actions are checked
against their screenshots; a successful capture alone is not a functional result.

The connected fixture checks actual UI password sign-in, server/channel loading,
encrypted sending, process restart and session restoration, and sign-out. It
asserts that the server received one `m.room.encrypted` event, that its content
contains no plaintext fixture, and that sign-out removes active session metadata.
Screenshots verify the decrypted text and the restored timeline. This is a
single-account UI integration check; the existing shared-core tests provide the
broader trust, plaintext refusal and retry checks.

Completed Linux checks on 2026-10-03:

- `scripts/check` passed formatting, Clippy with warnings denied, 3 desktop tests
  and 15 shared-core tests, plus desktop and headless native smoke checks.
  Six environment-dependent integration tests remain ignored in that command.
- The encrypted chat integration test passed separately against the local stack,
  covering restoration, retries, decryption, live channel indexing and the
  trusted-device/plaintext security gates.
- Private GUI captures covered Skia GPU at 1120×760 and Skia Software at 800×560.
  Connected captures passed encrypted sending, normal close, restored timelines
  and logout with both renderers. A long Matrix channel name elides within the
  240px sidebar at the minimum size.
- The independent finish reviewer scored the long-label containment fix resolved.
  This verdict covers the supplied Linux states; it does not establish Windows
  rendering, physical audio, IME or copy/paste behavior.

The final capture executable's SHA-256 is
`0f8cf20f8121f4e14254d301da800131ddf17c37f946a3444945b024fed5d7cd`.
The adapter tests include actual pointer dispatch into the navigation controls,
per-room drafts, secret clearing and disabled sending under the security gates.

The Windows all-target MSVC compilation check also passed after switching to the
static-runtime Skia archive. The production source builder passed its runtime,
symbol and COFF payload checks; its prepared archive SHA-256 is
`c233e623647195895a628e1dcf68353413dc9a4eb0b5d7ea4927fb5f3883fe4d`.
The Linux archive remains byte-identical to the one used by the reviewed captures.
The Windows release link, portable/source ZIP integrity checks, system-DLL import
check and private Wine 10.0 help/native smoke checks passed. The [Windows guide](windows-beta.md)
records the executable hash and the remaining actual-platform checks.

To reproduce the isolated checks after a desktop build:

```sh
nix develop path:./nix --command cargo build --locked
nix-shell --impure experiments/frameworks/shell.nix --run 'scripts/run-framework-isolated --compositor-log .local/slint-ui.weston.log -- python3 testing/probe_slint_ui.py --output .impeccable/review/slint-ui --renderer gpu --size 1120x760'
scripts/test-stack up
nix-shell --impure experiments/frameworks/shell.nix --run 'scripts/run-framework-isolated --no-services --compositor-log .local/slint-connected.weston.log -- python3 testing/probe_slint_ui.py --output .impeccable/review/slint-connected --renderer gpu --size 1120x760 --connected'
```

Use `--renderer software --size 800x560` for the minimum-size capture. GUI evidence
lives under the ignored `.impeccable/review` directory, with binary hashes and
assertion results in `actions.json`. Required checks finish with a headless build,
so rebuild with default features before starting a GUI probe.
Add `--connected --long-name` to exercise long labels against the local stack.

The previous two-machine messaging/audio report belongs to the GPUI beta.
The Slint package still needs actual Windows rendering, keyboard/IME/copy-paste,
resize, graphics compatibility and Linux–Windows audio checks by the tester.
Native audio/backend regression evidence does not prove the new desktop controls
on every platform. Full tray behavior, SSO/emoji verification, room creation,
ringing DM calls, video and screen sharing retain their existing future scope.
Attachments and the other conversation features are described in
[the chat guide](chat-ui.md). The renderer benchmarks here are fixture
measurements; [connected measurements](measurements/README.md) cover a real
account and calls.
