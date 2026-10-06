# Windows private beta

The portable package targets Intel/AMD PCs running 64-bit Windows 10 or 11.
The homeserver is prefilled as `https://matrix.org` and remains editable.
The current build uses Slint with Skia. Its cross-build and command-line checks
are recorded below. The 2026-10-02 manual and Wine reports describe the previous
GPUI package. Recheck the new UI on the tester's PC.

## Run the package

1. Copy `target/dist/Talk-0.1.0-windows-x64.zip` to the tester's PC.
2. Extract the whole ZIP, then double-click `talk.exe` in the extracted folder.
3. Sign in as `bob`, using the password supplied privately by the account owner.
4. Open **Talk beta**, then **general** for encrypted text or **Lounge** for voice.

The package needs no Rust or Visual Studio installation. It includes the native
media code and Skia renderer. A console window displays beta diagnostics.
Closing Talk closes that console too.
If it closes immediately, run `run-talk.cmd` to keep the error visible.
Talk renders with Skia Software. Use `run-talk.cmd --renderer gpu` to try the
GPU; if it cannot start, Talk switches back to Skia Software and says so in the
window.

Passwords, recovery keys and saved sessions are excluded from the package.
Session secrets use Windows Credential Manager. Encrypted SDK stores live under
the user's application-data directory. Keep the first verified session while
device verification and recovery are being tested.

## First two-machine check

With Alice signed in on Linux and Bob on Windows:

1. Exchange messages in general and confirm they survive restarting Talk.
2. Alice joins Lounge, stays muted and waits. Bob joins later and should see Alice.
3. Choose microphone and speakers in the voice settings. Use headphones.
4. Unmute both people and confirm each can hear the other.
5. Check mute, deafen, leaving, and rejoining. Browse general while voice stays connected.
6. Record Windows version, GPU/driver, audio devices, input/copy/paste behavior,
   window resizing, and any error text. Never include passwords or recovery keys.

The build and execution evidence is recorded in [compatibility](compatibility.md).
The user confirmed basic messaging and voice with the earlier GPUI package.
Repeat these checks with Slint. Exact Windows, GPU/driver and audio-device details
still need to be recorded.

## User-reported two-machine check on 2026-10-02

After launching Alice on Linux and the packaged Windows build for Bob, the user
confirmed message exchange, audio in both directions, mute/deafen and voice
remaining connected while browsing text channels. The Windows build also opened
successfully on the tester's Intel/AMD x64 PC.

These are user-reported manual results. Windows release, GPU/driver and audio
device models were not supplied. Restart behavior, extended waiting alone,
network interruption, forced crashes, resizing and copy/paste were not covered
by this confirmation.

## Slint build validation on 2026-10-03

The all-target Windows MSVC compilation check and release link passed. Skia was
built from pinned source with the static release CRT used by WebRTC; runtime,
symbol and native section checks passed before linking. The AMD64 PE import
table contains Windows system DLLs and no separate Visual C++ runtime DLL.

`--help` and `--smoke-test` passed under Wine 10.0 in a private prefix with private
XDG directories and no desktop display. The smoke check uses synthetic native
media and opens no microphone. These checks do not validate the Slint window,
Windows device selection, restored credentials or Linux–Windows audio.

The executable is 158.4 MiB, the source ZIP 64.4 MiB, and the portable ZIP about
121.5 MiB. The source includes the accumulated framework measurement artifacts.
Its implementation files, ZIP CRCs, embedded executable/source hashes, Skia
notices and exclusion of credential/session paths were checked. The executable
SHA-256 is `c381f8b5f734a68ab9b466f07582752dcf53b7ba9e387905afc7f039a4cc1847`.
`SHA256SUMS.txt` supplies the archive hashes, which change when documentation is
repackaged.

## Build validation on 2026-10-02

The release cross-build linked successfully as an AMD64 PE executable, with
static CRT linkage and all 18 GPUI shaders embedded. Its import table contains
Windows system DLLs and no separate Visual C++ runtime DLL. The portable ZIP
is about 52 MiB; its executable is about 139 MiB before compression.

Under Wine 11.12 Staging, `--help`, the native smoke check, password sign-in,
same-device restoration and audio-device enumeration all exited successfully.
The isolated check account signed in to the deployed homeserver, cross-signed
its device and stored credentials through Windows Credential Manager. Bob's
first SDK sign-in was preserved for the tester. Device enumeration opened no
microphone track.

The window check hit Wine's unimplemented font-matching API. The trace identifies
`dwritefontset_GetMatchingFonts_`, which returns `E_NOTIMPL` in the
[Wine 11.12 source](https://github.com/wine-mirror/wine/blob/wine-11.12/dlls/dwrite/font.c#L7071).
GPUI then cannot select a font. This prevents validating the Windows UI under
this Wine version. The subsequent manual Windows check is recorded above.

The full Linux check script passed after both dependency patches. ZIP CRCs,
executable/source hashes, patched-source inclusion and credential exclusion
were checked. The package includes no saved sessions, passwords or recovery keys.

## Rebuild from Linux

On this NixOS host, from the project folder:

```sh
nix develop path:./nix#windows --command scripts/build-windows
```

This installs the pinned Rust toolchain's Windows target, cross-compiles with
cargo-xwin and Clang, then packages the executable. The first run downloads the
Microsoft CRT/SDK and compiles the C++ media bindings. Cargo uses two jobs.

The Windows Nix shell supplies the matching `webrtc-51ef663` archive, LLVM 21,
GN, Protobuf and checksum-pinned Skia 0.153.3 source with its dependencies.
`scripts/build-windows` builds Skia with `/MT` to match WebRTC; the upstream Skia
Windows archive uses an incompatible dynamic CRT. This source build uses two
native jobs and is cached. Preparation then gives Skia's JPEG table a private
symbol name. It checks runtime directives and symbols, and preserves every
native section's code/data and relocations through the rename. The GPUI FXC/Wine
shader build is no longer part of this client.

A [one-line WebRTC patch](../vendor/webrtc-sys/README.talk.md) also fixes an
MSVC class/struct declaration mismatch, matching the upstream resolution.

The package contains dependency notices, original licenses, a source snapshot
and build hashes. `target/dist/SHA256SUMS.txt` covers both archives. The source
snapshot uses an explicit allowlist and excludes `.local`, account credentials,
sessions, logs and the Wine prefixes.
