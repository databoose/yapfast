# Talk

A native Rust and Slint client for Matrix chat and persistent voice channels.
The [product scope](PRODUCT.md) and [implementation plan](docs/implementation-plan.md)
describe the private beta.

The [application architecture](docs/architecture.md) separates the reusable
`talk-core` library from the Slint client. Other native UIs can share sessions,
chat and voice behavior, drafts, navigation and the local preview.

The [UI framework comparison plan](docs/framework-comparison-plan.md) covers
GPUI, Slint, Dioxus Native/Blitz, Freya, Iced and Dioxus WebView, with media
compatibility checks, memory-first benchmarks on Linux and Windows, and future
web/Android portability.

The desktop now has password sign-in, session restoration, a conversation list,
message history, device recovery and encrypted text sending. Voice channels have
join, leave, mute, deafen, participants and native audio device controls. Voice
stays connected while you browse messages. A local preview lets you try the
controls without an account or audio access.

Skia GPU is the default renderer. To use the optional Skia Software mode:

```sh
nix develop path:./nix --command cargo run --locked -- --preview --renderer software
```

Omit `--preview` to sign in or restore your real account. Renderer selection
does not change session storage, encryption or the native audio backend.

## Linux development

On this NixOS host, enter the pinned environment with:

```sh
nix develop path:./nix
cargo run --locked
```

To try the UI immediately, run this from the project folder:

```sh
nix develop path:./nix --command cargo run --locked -- --preview
```

Use the upper-left DM button for direct messages. Server buttons sit below it;
the sidebar shows only the selected server's channels. Filter that list, and send
local preview messages with Enter or the Send button. "Back to sign in" opens real Matrix sign-in. Normal launch
restores your saved session. Use `--data-dir PATH` for a separate test session.
Passwords and recovery keys are entered in masked native fields.

Real sending requires a verified device and an encrypted room. Verify Talk in
another client and press "Check again", or enter your existing recovery key.
Join rooms using another Matrix client for now. This first UI slice has no room
creation, channel discovery, ringing DM calls, attachments, or offline send queue. Drafts
survive conversation switches while Talk is open. Servers come from joined Matrix
Spaces, including channels in joined subspaces. Each server remembers its last
selected channel. Only joined channels appear; standalone rooms outside Spaces
are not shown in this navigation yet. Preview messages disappear
when you leave the preview, and closing the window quits Talk.

`path:./nix` also works before files are tracked and keeps Cargo output out of
the Nix source snapshot. The shell provides Clang 21, Protobuf, desktop libraries,
and checksum-pinned libwebrtc and Skia archives. The Skia archive keeps the
verified JPEG symbol namespace needed to link it alongside WebRTC. Rustup uses
the version in `rust-toolchain.toml`.

```sh
scripts/check
cargo run --locked --no-default-features -- --smoke-test
cargo build --locked --release
```

With the local stack running, check the desktop messaging backend with:

```sh
cargo test --locked -p talk-core chat::tests::encrypted_chat_restore_send_retry_and_security_gates -- --ignored
```

This creates disposable accounts and checks session restoration, encrypted
delivery, duplicate-free retry, Space membership and live channel removal, and
refusal of plaintext rooms and unverified devices. Current native UI validation
is in [the Slint client report](docs/slint-client.md); the earlier GPUI checks
remain in [the chat UI report](docs/chat-ui.md).

Run those commands inside the Nix shell. The headless build still includes the
Matrix and native media libraries. The smoke check allocates a synthetic audio
source. It does not sign in, connect to a server, or access the microphone.

## Local integration services

With a running Docker daemon and Compose v2:

```sh
scripts/test-stack up
scripts/test-stack check
scripts/test-stack down
scripts/test-stack reset
```

`reset` deletes only this stack's disposable database and certificate volumes,
then restarts the services. The services bind to loopback and use test credentials.
They are for development on one machine. Remote testers require a separate TLS
deployment with reachable media ports.

The check provisions `alice`, `bob`, and `charlie` on `synapse`, with the password
`talk-test-only`. It verifies homeserver capabilities and a real Matrix OpenID to
LiveKit authorization exchange, including rejection of an invalid token. It does
not prove audio encryption or room-membership authorization.

| Service | Local address |
| --- | --- |
| Matrix homeserver | `http://localhost:18008` |
| MatrixRTC authorization | `http://localhost:16080` |
| LiveKit | `ws://localhost:17880` |

## Sessions and native call experiment

With the local services running:

```sh
cargo run --locked --no-default-features -- --call-experiment
```

This creates fresh disposable accounts, checks restoration and recovery, then
tests audio in both directions, invalid keys and key rotation after a participant
leaves. It uses synthetic PCM and accesses no microphone or speaker.
The [experiment report](docs/native-call-experiment.md) records its evidence and limits.

Run `cargo run --locked --no-default-features -- --help` for session commands.
[Session instructions](docs/sessions.md) cover hidden password/key prompts, secure
storage, logout retention and current authentication coverage.

## Windows development

Build a portable Windows x64 ZIP from this Linux checkout with:

```sh
nix develop path:./nix#windows --command scripts/build-windows
```

The package appears in `target/dist`. Extract it on Windows and open `talk.exe`.
The sign-in screen defaults to `https://matrix.org`. See the
[Windows beta guide](docs/windows-beta.md) for account and two-machine checks,
and [building](docs/building.md) for native Windows prerequisites.

[Compatibility](docs/compatibility.md) records dependency pins and what has
actually been tested. [Development status](docs/development-status.md) tracks the
remaining milestone exit criteria.

## Shared beta server

The shared private beta uses `https://matrix.org`, with public voice
services and closed registration. [Deployment and connection instructions](deploy/coolify/README.md)
cover the two beta accounts, saved Alice session, Coolify configuration and
remaining two-machine voice checks.

## Try voice locally

Inside the development shell:

```sh
scripts/test-stack up
cargo run --locked -- --voice-demo
```

The demo prints commands for two saved test accounts. Open them in separate
processes, select **Talk voice test → Lounge**, and press **Join voice**. Each
joins muted. The gear button selects microphone and speakers. Press Unmute to
talk, or stay muted and wait for a friend. The [voice guide](docs/voice.md) covers
requirements, checks and the Windows test.

The [resource measurements](docs/measurements/README.md) record their workloads
and limits. Beta resource targets remain unverified.
