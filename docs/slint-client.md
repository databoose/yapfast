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
- The existing neutral dark layout, Lucide SVG icons, native text editing and
  window sizing from 1120×760 down to 800×560. Audio devices use a popover above
  the footer, dismissed by Close, Escape or clicking outside.

`ui/talk.slint` defines the controls and layout; `src/ui.rs` adapts application
state and effects. Backend events wake Slint's executor through the Tokio
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

Omit `--preview` for normal sign-in or session restoration. Renderer choice is
explicit for this slice. Startup failures are reported; automatic GPU-to-software
recovery remains open and is not established by Slint's generic backend fallback.
The current app uses stock Slint renderer code, without benchmark timing hooks.

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
ringing DM calls, attachments, video and screen sharing retain their existing
future scope. The renderer benchmarks remain fixture measurements; no connected
Slint idle/call memory target is claimed here.
