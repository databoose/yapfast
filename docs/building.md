# Native builds

## Dependencies

Talk pins Rust 1.97.1, Slint 1.18.1 and Skia 0.153.3. Cargo.lock pins
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
are required to open the window. Skia Software is the default; `--renderer gpu` uses Skia GPU and falls back to
Skia Software if the GPU fails. The Nix shell provides a checksum-pinned,
prepared Skia archive whose JPEG table is privately named to avoid a WebRTC
symbol collision. See [the Slint client](slint-client.md).

The first build downloads dependencies and compiles native C++. Cargo uses every
core. On a machine with 8 GB of memory, set `CARGO_BUILD_JOBS=2` so the Matrix
SDK and the C++ media bindings do not exhaust it.

Development builds omit debug symbols for dependencies; the app keeps line
tables. Release builds use thin LTO and strip symbols. Use them for builds
people run and for measurements. The `fast` profile optimizes like release
without LTO and builds incrementally. Use it for tests and local runs:
`cargo build --profile fast` or `cargo test --profile fast`.

With [direnv](https://direnv.net/), the repository's `.envrc` loads the Nix
development shell when you enter the folder, so commands need no
`nix develop` prefix. On NixOS, `programs.direnv.enable = true;` installs direnv
with nix-direnv, which caches the shell.

## Windows

The Linux cross-build and portable ZIP route is documented in the
[Windows beta guide](windows-beta.md). It supplies the matching Windows WebRTC
archive and checksum-pinned Skia source through the Windows Nix shell.
`scripts/build-windows` prepares and caches a static-runtime Skia build before
compiling Talk. The executable uses static CRT linkage to match LiveKit.
GPUI's FXC shader build is no longer used.

The supported build route for this migration is the Linux MSVC cross-build.
Native Windows development needs Rustup, Visual Studio C++ tools and SDK, CMake,
LLVM and Protobuf, plus the same prepared Skia archive through
`SKIA_BINARIES_URL`. That native-host preparation/build route has not been checked.
The downloadable package needs none of these tools to run.

Before accepting the Windows build, check text entry, selection, copy/paste,
keyboard focus, navigation, both renderers, resizing, normal close, and reopen
on the tester's actual GPU. Record Windows and GPU versions in compatibility.md.
Windows execution has not been demonstrated by a Linux build.

## Logs

Talk logs only its own operational events at info level and above. Dependency
logs are disabled because they can expose authentication data or message content.
Typed text never enters the log. Future errors must use fixed event
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
room counts, renderer, build mode, warm-up, window size and activity alongside
the output. The earlier empty GPUI build-check baseline had no account or sync;
it does not measure the current connected Slint client.

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
