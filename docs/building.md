# Native builds

## Dependencies

Talk pins Rust 1.97.1, GPUI Kit 0.7.0 and its GPUI 0.3.7 snapshot. Cargo.lock pins
the complete Rust graph. The Matrix SDK revision follows the pinned MatrixRTC
revision. Keep those two sources together when upgrading.

LiveKit's Rust crates must stay on the mutually compatible versions in Cargo.lock.
Do not run an unrestricted `cargo update`. Upgrade against the MatrixRTC lockfile
and rebuild the native media stack. On Linux, the Nix shell supplies libwebrtc
`webrtc-51ef663`, matching webrtc-sys-build 0.3.18. Updating that crate requires
updating the archive URL and checksum in nix/flake.nix too.

The Linux environment includes ALSA, PulseAudio, fontconfig, FreeType, GLib, Wayland, X11,
Vulkan, OpenGL, OpenSSL, Clang 21, CMake, pkg-config and Protobuf. It uses the
host's graphics driver under `/run/opengl-driver/lib`. A real display session
and a GPU supported by GPUI's renderer are required to open the window.

The first build downloads dependencies and compiles native C++. Cargo limits
parallel jobs to two to limit memory pressure. Development dependencies omit
debug symbols; the app keeps line tables. Release builds use thin LTO and strip
symbols.

## Windows

The Linux cross-build and portable ZIP route is documented in the
[Windows beta guide](windows-beta.md). It supplies the Windows WebRTC archive
and SDK shader compiler through the Windows Nix shell. The executable uses
static CRT linkage to match LiveKit and embeds GPUI's release shaders.

Build on 64-bit Windows using Rustup and the MSVC target. Install Visual Studio
2022 Build Tools with Desktop development with C++, a Windows SDK, CMake, LLVM,
and Protobuf. Include the SDK's FXC shader compiler for release builds. Run from
a Developer PowerShell where `cl`, `clang`, `cmake`, and
`protoc` are available. Confirm with:

```powershell
rustup show
Get-Command cl, clang, cmake, protoc
./scripts/check.ps1
cargo build --locked --release
./target/release/talk.exe
```

webrtc-sys-build downloads the matching Windows libwebrtc archive on its first
build. It is version-pinned by Cargo.lock. Linux uses a Nix fixed-output fetch
instead so the C++ build script needs no network access inside the Nix sandbox.

Before accepting the Windows build, check text entry, selection, copy/paste,
keyboard focus, the background-task button, resizing, normal close, and reopen
on the tester's actual GPU. Record Windows and GPU versions in compatibility.md.
Windows execution has not been demonstrated by a Linux build.

## Logs

Talk logs only its own operational events at info level and above. Dependency
logs are disabled because they can expose authentication data or message content.
The build-check field never enters the log. Future errors must use fixed event
names and safe classifications rather than raw protocol responses or SDK Debug
output.

## Initial resource measurement

After a release build, open `target/release/talk` inside the Nix shell. Leave it
inactive for at least 15 seconds, then sample its PID on Linux:

```sh
scripts/measure-process PID --seconds 30
```

The JSON reports mean and peak RSS, mean PSS, CPU use relative to one core,
and GPU engine counters when the driver exposes them through `/proc/PID/fdinfo`.
An unavailable GPU metric is reported explicitly. Record hardware, account and
room counts, warm-up, window size and activity alongside the output. The current
empty build-check window has no account or sync and does not measure beta idle.

## Local backend

Synapse and the authorization service communicate over an internal TLS listener
using a throwaway self-signed certificate. LiveKit and authorization share a
network namespace, and use the same signaling port inside and outside Docker.
Authorization uses the standalone `/get_token` mode from lk-jwt-service 0.7.0.
Talk's native experiment sets the local authorization fallback explicitly,
because this stack does not advertise a production RTC discovery endpoint.
See [the experiment](native-call-experiment.md) for its command and evidence,
and [sessions](sessions.md) for credential storage and headless sign-in.

The service only allows the local `synapse` homeserver to create SFU rooms. This
does not establish a production friend-group or room-membership access policy.
That policy remains part of the encrypted calling experiment.
