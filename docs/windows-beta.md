# Windows private beta

The portable package targets Intel/AMD PCs running 64-bit Windows 10 or 11.
The homeserver is prefilled as `https://matrix.org` and remains editable.
The current build uses Slint with Skia. Its cross-build and command-line checks
are recorded below. The 2026-10-02 manual and Wine reports describe the previous
GPUI package. Recheck the new UI on the tester's PC.

## Run the package

1. Copy `target/dist/Yapfast-0.1.0-windows-x64.zip` to the tester's PC.
2. Extract the whole ZIP, then double-click `yapfast.exe` in the extracted folder.
3. Sign in as `bob`, using the password supplied privately by the account owner.
4. Open **Yapfast beta**, then **general** for encrypted text or **Lounge** for voice.

The package needs no Rust or Visual Studio installation. It includes the native
media code and Skia renderer. Opened from Explorer, Yapfast shows only its
window and writes its diagnostics to `yapfast.log` in its data folder,
`%LOCALAPPDATA%\Yapfast\Yapfast\data`, or the `--data-dir` folder. The log grows
across starts and starts over past 4 MiB.
Started from a console, as `run-yapfast.cmd` does, Yapfast writes there instead.
If it closes immediately, run `run-yapfast.cmd` to keep the error visible.
Yapfast renders with Skia Software. Use `run-yapfast.cmd --renderer gpu` to try the
GPU; if it cannot start, Yapfast switches back to Skia Software and says so in the
window.

Passwords, recovery keys and saved sessions are excluded from the package.
Session secrets use Windows Credential Manager. Encrypted SDK stores live under
the user's application-data directory. Keep the first verified session while
device verification and recovery are being tested.

## First two-machine check

With Alice signed in on Linux and Bob on Windows:

1. Exchange messages in general and confirm they survive restarting Yapfast.
2. Alice joins Lounge, stays muted and waits. Bob joins later and should see Alice.
3. Choose microphone and speakers in the voice settings. Use headphones.
4. Unmute both people and confirm each can hear the other.
5. Check mute, deafen, leaving, and rejoining. Browse general while voice stays connected.
6. Choose a camera in the voice settings, try "Test video", then turn the camera
   on in Lounge. Each should see the other's camera. Restart Yapfast and check the
   camera is still chosen.
7. Record Windows version, GPU/driver, audio devices, cameras, input/copy/paste behavior,
   window resizing, and any error text. Never include passwords or recovery keys.

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

## Discord voice on Windows

Discord voice, the input meter and the Discord input sensitivity meter,
ringing, Yapfast's sounds, and a watched stream's or a video's sound use WASAPI
on Windows, in `crates/yapfast-core/src/audio_io/wasapi.rs`. Each stream is
shared-mode and event-driven: the audio engine converts between Yapfast's 48 kHz
and the device's format, and reading or writing waits for its event on the
thread that already did the reading or writing on Linux, copying straight
between Yapfast's buffers and the engine's. WebRTC's Windows device module names
each device by its endpoint id (`{0.0.0.00000000}.{…}` for speakers,
`{0.0.1.00000000}.{…}` for microphones), so the chosen device opens directly
by that id; none is the console default. Go Live's sound is in
[Go Live on Windows](#go-live-on-windows).

Checked under Wine 10.0 on 2026-10-13, with Wine's audio going to a private
PulseAudio of null sinks, so nothing reached real speakers:

- `audio_io::tests::hears_what_it_plays`: a second of a 440 Hz tone played
  through WASAPI is heard back whole (876–879 zero crossings for 880).
- `audio_io::tests::opens_every_listed_device`: every microphone and speaker
  LiveKit lists opens by its id, and an unknown id is refused.
- `discord_audio::processes_a_recording`: the Discord microphone's processing,
  WebRTC's echo cancellation and gain control with DeepFilterNet, gives the
  same bytes as on Linux.
- `discord_audio::tests::a_call_sends_what_the_microphone_hears`, against
  `testing/fake_discord.ts`: Yapfast joins the Lounge, unmutes, and the fake
  receives 56–91 Opus packets that aren't silence from two seconds of beeps,
  with the speaking flag, in three runs out of three.
- The rest of `yapfast-core`'s tests pass, except
  `frames::tests::frames_pass_from_writer_to_reader`, which
  [Go Live on Windows](#go-live-on-windows) fixed.

Run them from the Windows shell with `cargo xwin test --no-run --profile fast
--target x86_64-pc-windows-msvc -p yapfast-core --lib`, then run the test
executable under Wine with `PULSE_SERVER` naming the private PulseAudio.

Wine's capture starts 0.3–1 s after it's opened, so the first moments a
microphone hears are lost there. Real Windows needs checking for that, along
with Bluetooth headsets, a device unplugged during a call, a default device
changed during a call, exclusive-mode applications, and whether the
communications ducking setting turns other sound down during a call.

## Go Live on Windows

Sharing, watching and playing a video run in the stream worker, as on Linux:
Yapfast starts its own executable with `--stream-worker`, which writes its logs
where Yapfast does: Yapfast's console, or Yapfast's log file. The video frames reach
the window through a temporary file both processes map. Windows won't delete
a mapped file, so there Yapfast creates it with `FILE_FLAG_DELETE_ON_CLOSE` and
`FILE_ATTRIBUTE_TEMPORARY`, which keeps it in memory while it fits, and
Windows removes it once both processes close it. On Linux it's still unlinked once the worker has it open.

Windows has no screen sharing dialog for Yapfast to use, so the share menu
lists what there is to share above the resolution: the screens as pictures,
two to a row, named "Entire screen" (or "Screen 1", "Screen 2", … with
several), then each window with a title. Yapfast's own windows aren't listed.
libwebrtc's capturers make the list, on a short-lived thread as the menu
opens, and the worker finds the picked one by its id. The menu opens with
the screens' names; their pictures come from a worker job (`Job::Glance`)
that takes one frame of each screen, scaled down to its card, and exits, so
capturing them takes memory only until they're in. A window closed after it was listed ends sharing with "That screen
or window is gone." Without a pick, Yapfast shares the first screen, as before.

Go Live's sound comes from WASAPI process loopback: one stream of everything
Windows plays except Yapfast's own process tree, the Yapfast window and its
workers. It needs Windows 10's 2004 update or later. Unlike Linux, where
each application is heard on its own and Discord's own client is left out,
Windows mixes every other application into one stream and can leave out only
Yapfast. Where Windows refuses process loopback, Yapfast shares without sound and
logs why, as it does on Linux without a PulseAudio server.

Checked under Wine 11.18 Staging on 2026-10-13:

- `frames::tests::frames_pass_from_writer_to_reader` passes, and the file is
  gone once both sides close it.
- `stream_worker::tests::shares_a_listed_screen_and_window`, on a private X
  display with Notepad open: the list holds the screen and "Untitled -
  Notepad". The screen shares at 1280×720 with Notepad in the picture (35% of
  it not black). The window shares at Notepad's own 684×512, though black:
  Wine's GDI window capture gives no pixels without a compositor, where
  Windows uses Windows Graphics Capture. A window id that's gone ends with
  the message above.
- `audio_io::tests::hears_other_applications` shows only the fallback: Wine
  has no `VAD\Process_Loopback` device, so starting the capture fails with
  "File not found" and sharing goes on without sound. On Linux the same test
  hears a tone played by another process (880 zero crossings for 880) and
  not the one it plays itself.
- The Discord-only `yapfast.exe` (`--no-default-features --features
  desktop,discord`) on `scripts/ui-session --discord`, whose `--bin` was a
  script that unsets `DISPLAY`, so Wine uses its Wayland driver, starts
  Notepad and Yapfast with `wine explorer /desktop=yapfast,1280x800`, and waits on
  `wineserver -w`. Without the Wine desktop, Weston's kiosk shell gives
  Notepad the pointer from behind Yapfast. Yapfast joins the
  Lounge on the fake Discord, and the share menu opens above the button with
  "Entire screen" and "Untitled - Notepad", leaving out Yapfast's console.
  Going live with Notepad starts the worker; the fake receives 1,064 H264
  frames at 1280×800 with 12 keyframes and none undecryptable, and no sound.
  Stopping ends the worker and leaves no frames file in the temporary folder.
  The preview is black there too, since Wine's Wayland driver can't read
  windows back.

Under Wine with only an X server, Slint panics before the window opens:
Xvfb reports its monitor at 0 Hz, and Slint's winit backend divides by the
refresh rate (`frame_throttle.rs`). A Windows display that reports no
refresh rate would hit the same, so check one under remote desktop or in a
virtual machine.

Real Windows needs checking for: the window list's contents and titles,
sharing a window that's minimized, moved to another screen or closed while
shared, screens on mixed-DPI setups, Windows Graphics Capture's yellow
border, process loopback hearing other applications and leaving out Yapfast's
own voices, and the worker's start-up time from a packaged build.

## Cameras on Windows

Cameras film through libwebrtc's DirectShow capture; see
[Cameras](voice.md#cameras) for how, and how a camera's id is kept.

Checked under Wine 10.0 on 2026-10-14. That Wine is built without libv4l2,
so its DirectShow reads only a V4L2 device that supports `read()` and BGR24:
a v4l2loopback device fed by `ffmpeg -re -f lavfi -i
testsrc2=size=1280x720:rate=30 -pix_fmt bgr24 -f v4l2 /dev/videoN`. It also
lists a USB webcam, which then fails to open.

- `camera::tests::the_test_pattern_films_until_dropped` passes.
- `films_a_real_camera` with `YAPFAST_TEST_CAMERA` naming the loopback films 32–34
  frames in a second at 1280×720. A frame dumped from it shows the picture
  the right way up, not mirrored, and in its colours.
- The packaged `yapfast.exe` passes `--help` and `--smoke-test`, and
  `--audio-devices` lists the test pattern, the webcam and the loopback.
- The Discord-only `yapfast.exe` on `scripts/ui-session --discord`, through a
  `--bin` script like Go Live's below. Settings list the cameras, "Test
  video" shows the loopback, and the choice is still there after Yapfast
  restarts. In a DM call with Bob on the fake Discord, the webcam that won't
  open says "Could not start your camera" and the call goes on. With the
  loopback, the fake receives 671 H264 frames at 1280×720 with 8 keyframes
  and none undecryptable; Bob's camera shows in his tile; turning the camera
  off stops it, and the `yapfast-camera` thread and Wine's capture threads go.
- The full `yapfast.exe` in a Matrix call on the local stack (`scripts/test-stack
  up`), signed in as a new account whose first device it is, so it's
  verified: the camera publishes a 1280×720 VP8 track with three simulcast
  layers to LiveKit, and a Linux Yapfast in the same call shows it moving. The
  Windows build shows the Linux participant's camera too.

Only real Windows can check a real camera through DirectShow's kernel
streaming (MJPEG, YUY2 and NV12, and the capability chosen for 1280×720 at 30
frames a second), that a camera's id survives restarts and replugging, COM on
the threads that list and open cameras, a camera another application is
using, the camera privacy setting turned off, built-in laptop cameras that
only Media Foundation lists, and the memory and processor time a camera
takes.

## The tray on Windows

The notification-area icon is `Shell_NotifyIconW` on a hidden window of the
UI thread; see [the tray](tray.md#windows).

Checked under Wine 11.18 Staging on 2026-10-14, with the Discord-only
`fast` `yapfast.exe` on `scripts/ui-session --discord`, through a `--bin`
script that runs it in `wine explorer /desktop=yapfast,1120x760` and sends its
output to a file. Without the Wine desktop, winit can't create its window
there (OS error 14007).

- Closing the window drops it, and Wine's notification area, a window of its
  own in Weston's kiosk shell, shows the marigold logo with the unread dot.
- A left click on the icon opens the window again.
- A right click reaches Yapfast (`WM_RBUTTONDOWN`, `WM_RBUTTONUP`) and
  `TrackPopupMenu` runs, but Wine's Wayland driver never shows the menu,
  most likely because its owner is a window that was never shown. Until the
  menu closes, its loop holds Yapfast's UI thread, as every Win32 menu does; on
  Windows a click anywhere else closes it. So Open Yapfast, Disconnect and Quit
  in the menu are untested on Windows.

[The tray](tray.md#windows) lists what to check on real Windows.

## Dragging the window

From the press on the title bar or a border until the release, Windows
moves or sizes a window in a message loop of its own. winit still passes
window messages on, but its own loop doesn't run, and Slint advances its
timers and animation clock only when it does, so the window looked frozen
while dragged: animations stood still, and pictures that arrived waited.
`src/ui/move_loop.rs` starts a Win32 timer with the first move or resize
winit reports while Windows is moving or sizing the window
(`GetGUIThreadInfo`, `GUI_INMOVESIZE`). Windows calls it from its own loop
every 16 ms, and it advances Slint's timers and animations and redraws what
animates, until the window stops moving. `YAPFAST_LOG=yapfast=debug` logs when it
starts and stops.

Wine doesn't reproduce the freeze. In its own desktop (`wine explorer
/desktop=…`), the typing indicator kept pulsing while the old build was
dragged by its title bar. So check on real Windows: while the window is
held by its title bar, still and moving, "… is typing" should keep pulsing
and its dots rising; the same while resizing by a border.

## Build validation on 2026-10-13

The first cross-build since Discord voice, Go Live's native codecs and
DeepFilterNet landed failed in one place: songbird's `opus2` builds its own
libopus, which clang-cl couldn't compile (its SSE4.1 files need flags CMake
gives only GCC-style compilers) and whose symbols would clash with the libopus
inside libwebrtc's archive. The Windows Nix shell now gives `opus2` an empty
`opus.lib` through `LIBOPUS_LIB_DIR`, so it uses libwebrtc's copy and the
executable holds one libopus. Linux is unchanged. Since Discord calls moved
onto Yapfast's own voice connection, nothing builds `opus2`: Yapfast calls
libwebrtc's libopus directly (`yapfast_discord::opus`), and the shell's empty
library is gone.

`--smoke-test` now encodes 100 ms of a tone with Opus, now through
`yapfast_discord::opus`, and decodes it again. It and `--help` passed under Wine 10.0 in a private prefix
without a display, for both the `fast` and the release build. The release
executable is 211.3 MiB and the portable ZIP 141.3 MiB; Discord voice,
DeepFilterNet's model and Go Live's codecs account for the growth since
2026-10-03. The executable's SHA-256 was
`a8d11fbea3c369b963d4c8c513cf422c5239f06e97005b0d069eba0aca2f5286`.

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

A [one-line WebRTC patch](../vendor/webrtc-sys/README.yapfast.md) also fixes an
MSVC class/struct declaration mismatch, matching the upstream resolution.

The package contains dependency notices, original licenses, a source snapshot
and build hashes. `target/dist/SHA256SUMS.txt` covers both archives. The source
snapshot uses an explicit allowlist and excludes `.local`, account credentials,
sessions, logs and the Wine prefixes.
