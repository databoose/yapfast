# Native builds

## Shells

Three Nix shells cover the work:

- `nix develop path:./nix` is the development shell. Run every `cargo` command
  in it, as `nix develop path:./nix --command cargo ...`. It also provides
  `bun` for the fake Discord.
- `nix-shell --impure testing/display/shell.nix` adds Weston,
  PulseAudio, the ALSA plugins and FFmpeg to it, for the private display that
  `scripts/run-framework-isolated` starts.
- `nix develop path:./nix#windows` cross-builds for Windows; see the
  [Windows beta guide](windows-beta.md).

The repository's own `flake.nix` builds no shell, only the package that
`nix profile add github:yapfast/yapfast#yapfast` installs: the Discord-only
release build, from `nix/package.nix`. It shares libwebrtc, Skia and the
system libraries with the development shell through `nix/native.nix`, and pins
the same nixpkgs in its own `flake.lock`. `nix build` puts it in `result`.
It also builds the AppDir that the [AppImage](#linux-appimage) is packed from.
`nix/package.nix` holds one hash per Git dependency's repository and commit,
in `outputHashes`. Moving a Git dependency to another commit needs a new
hash there; a `nix build` with the old one prints it.

## Dependencies

Yapfast pins Rust 1.97.1, Slint 1.18.1 and Skia 0.153.3. Cargo.lock pins
the complete Rust graph. `vendor/i-slint-renderer-skia` is Slint's Skia
renderer 1.18.1 with these changes. Glyphs use Skia's slight (vertical-only)
hinting, because the default hinting spaced Satoshi's letters unevenly.
Smooth pictures up to 512×512 pixels sample mipmaps, because plain bilinear
sampling made pictures drawn smaller than their pixels, such as avatars, look
jagged. Larger ones, such as video frames, go without: each frame would build
its mipmaps anew, 4 ms more for a 1080p frame. So do pictures drawn no
smaller than their pixels, such as a call's camera, made at its tile's size.
Skia reads fonts where fontique keeps them rather than copying each file to
the heap, which took 10 MiB for the colour emoji font. It reads a picture made
in code, such as a camera's frame or an avatar, where Slint keeps it too,
rather than copying it for each new frame: a table on the UI thread keeps the
Slint buffer beside Skia's image until nothing else holds that image, and
Slint copies a buffer it shares before anything writes to it. On Linux it
takes rust-skia's prebuilt Skia with Skia's own FreeType: the one built for
the system's FreeType lacks FreeType's COLRv1 calls, so it drew nothing for the
COLRv1 build of Noto Color Emoji, the only one Fedora installs. Rust-skia
builds that Skia only with its text layout, which the linker leaves out, and
without Vulkan, so the renderer goes without wgpu, whose Vulkan surface needs
it. On Linux Yapfast asks for Skia OpenGL or Skia Software by name; the wgpu
renderers the winit backend still names make surfaces that fail to start.
Without wgpu the Discord-only binary is 4.2 MB smaller. Windows turns wgpu back
on through a dependency in Yapfast's own `Cargo.toml`, for Skia GPU on
Direct3D 12 (see `src/graphics.rs`). Each change is marked `Yapfast:` in
`itemrenderer.rs`, `cached_image.rs`, `lib.rs`, `font_cache.rs` or
`Cargo.toml`. Copy the matching release over it, and reapply them, when
upgrading Slint.

`vendor/i-slint-core` is Slint's core 1.18.1 with `StyledText` made selectable,
which upstream Slint doesn't offer. Dragging selects, a double press selects a
word and a triple press a paragraph. Dragging past a text carries the selection
on through the texts and pictures around it: those sharing its nearest flickable,
such as a list's rows, in reading order. Pictures are the `Image`s with
an `accessible-label`, and copy as `[Picture]`. Ctrl+C copies the selection
as read, without its Markdown: texts level with each other joined by a space, the
rest by line breaks. Yapfast shows message authors and times as `StyledText` so the
selection takes them in. The selection is kept as positions in the list's model,
so it survives its rows scrolling away. Dragging near the top or bottom of the
list scrolls it, and Yapfast supplies the text of the rows in between through
`yapfast_selection::set_row_text`. A click that selects nothing still follows a
link and leaves the focus where it was. A left press anywhere clears the
selection, as does Escape. Other buttons pass through a text to the items around
it, so a right click on a message's text opens its menu, and
`yapfast_selection::copied_at` gives that menu the selection there, and
`yapfast_selection::pressed_link_at` the link pressed. The highlight is the text's `link-color` at 35%
opacity, since `StyledText` has no property of its own for it.

The core of it is `yapfast_selection.rs`. The other changes are marked `Yapfast:` in:

- `items/text.rs`
- `items/image.rs`, which tints covered pictures
- `items/flickable.rs`, which keeps the pointer through a drag
- `window.rs`, for the press, Ctrl+C and Escape
- `item_rendering.rs`
- `textlayout/sharedparley.rs`
- `textlayout/sharedparley/shaping.rs`, where a link in a colour shows in it,
  without an underline, on a pill only when it's a mention, to `yapfast:user/`,
  `yapfast:role/` or `yapfast:everyone`, and where the font family `monospace`
  is the system's monospace font, as inline code's, for fenced code
- `textlayout/sharedparley/draw.rs`, which draws mention pills under their
  text and opaque covers over hidden Discord spoilers, links to `yapfast:spoiler/`
- `graphics/image.rs`, where no image equals no image; upstream they differ,
  so every list row without a picture counted as changed on each refresh

It also keeps a timeline's rows in place as older messages go in above them.
Upstream, a `ListView` keeps its rows where they are on screen when rows go in
above the first one it shows, but rows that go in right above it, as when the
top of the history is on screen, push everything shown down. Inside
`yapfast_anchor::hold`, which Yapfast wraps around a page of older messages, those
count as above it too. That row then keeps its bottom edge where it is in the
view, rather than its top, while its height settles: it loses its date, its
author line or the loading notice once the rows before it carry them, and
those parts change only when the list is next drawn. The anchor moves with
scrolling, follows its row as rows go in or out above it, and ends when the
row is no longer shown or goes. A wheel scroll animates
`content-y` and would carry on to where it was going, so the list stops that
animation when rows go in above inside `hold`. This is `yapfast_anchor.rs` and the
changes marked `Yapfast:` in `model/repeater.rs`.

A `TextInput` draws parts of its text as a `StyledText` draws a mention, in
a colour on a pill of it, so the composer shows whom its message will
mention. Yapfast marks a text's byte ranges with
`yapfast_marks::mark`, and a `TextInput` showing that text, not a password
or one composing with an input method, shapes them as links in a colour span.
Upstream, a `TextInput` has one colour. This is `yapfast_marks.rs` and the
changes marked `Yapfast:` about marks in `item_rendering.rs`, `items/text.rs`
and `textlayout/sharedparley/shaping.rs`.

A mouse wheel spun fast scrolls farther for each notch. Upstream, each notch
scrolls 60 pixels however fast the notches come, so a hard spin through a long
history crawls. Yapfast weighs the recent notches, each counting less with time,
and above 12 notches a second scales each notch by the speed over 12, at most
six times. Deltas smaller than a notch, as from touchpads that send no phases,
keep their pace. These are the changes marked `Yapfast:` around
`accelerate_wheel` in `items/flickable.rs`.

A middle click autoscrolls, as browsers do: the innermost flickable under it
with more to show than fits scrolls towards the pointer, faster the farther
away the pointer is from where it clicked, until the next press, key or wheel.
Holding the button and moving away scrolls until it's let go. A middle press
in a `TextInput` still pastes there. While it scrolls the window gives the
pointer's events to it, not to the items under the pointer, and the press
that ends it goes no further. Yapfast marks where it began through
`yapfast_autoscroll::on_marker`, with the `Autoscroll` global in
`ui/scrollview.slint`. Upstream Slint has no autoscroll. This is
`yapfast_autoscroll.rs` and the changes marked `Yapfast:` about it in
`items/flickable.rs`, `items/text.rs` and `window.rs`.

Reapply them to the matching release when upgrading Slint. Styled paragraphs now
have byte ranges as if joined by newlines, where upstream gives them all `0..0`.

`ui/scrollview.slint` is Yapfast's own `ScrollView` and `ListView`, in place of
the std-widgets ones: the Fluent style's from Slint 1.18.1, with its scroll
arrows copied to `ui/icons/scroll-*.svg`. The Fluent scrollbar starts a thumb
drag over each time `maximum` changes, dropping part of the pointer's travel,
and a `ListView`'s `maximum` changes all through a drag as rows are measured,
so the thumb fell behind the pointer. Yapfast's bar sets the viewport from where
the pointer has moved the thumb since the drag began, as a share of the track,
and again when `maximum` changes. The compiler virtualizes any component named
`ListView` with the viewport's lengths, so Yapfast's is virtualized as upstream's
is. Compare it with the Fluent style's when upgrading Slint.

The Matrix SDK revision follows the pinned MatrixRTC revision. Keep those two
sources together when upgrading.

libDF, DeepFilterNet's Rust implementation, comes from DeepFilterNet's
repository at `d375b2d`, since crates.io has only its 0.2.5. It asks for
tract `^0.21.4`, but tract 0.21.10 changed an API it uses, so Cargo.lock keeps
tract at 0.21.4, the version DeepFilterNet's own lockfile has.

LiveKit's Rust crates must stay on the mutually compatible versions in Cargo.lock.
Do not run an unrestricted `cargo update`. Upgrade against the MatrixRTC lockfile
and rebuild the native media stack. On Linux, the Nix shell supplies libwebrtc
`webrtc-51ef663`, matching webrtc-sys-build 0.3.18. Updating that crate requires
updating the archive URL and checksum in nix/flake.nix too.

The Linux environment includes ALSA, PulseAudio, fontconfig, FreeType, GLib, Wayland, X11,
Vulkan, OpenGL, OpenSSL, Clang 21, CMake, pkg-config, Protobuf and NASM. It uses the
host's graphics driver under `/run/opengl-driver/lib`. A real display session
are required to open the window. Skia Software is the default; `--renderer gpu` uses Skia GPU and falls back to
Skia Software if the GPU fails. The Nix shell provides a checksum-pinned,
prepared Skia archive whose JPEG table is privately named to avoid a WebRTC
symbol collision. See [the Slint client](slint-client.md).

The first build downloads dependencies and compiles native C++. Cargo uses every
core. On a machine with 8 GB of memory, set `CARGO_BUILD_JOBS=2` so the Matrix
SDK and the C++ media bindings do not exhaust it.

Development builds omit debug symbols for dependencies; the app keeps line
tables. They optimize DeepFilterNet's crates and turn off tract-core's debug
assertions, which reject its model; see Cargo.toml. Release builds strip
symbols and use no LTO, which made both platforms' binaries bigger for no
measured speed. Use them for builds people run. The `measure` profile
optimizes like release, keeps symbols and builds incrementally. Use it for
[measurements](measurements/README.md) and profiling: `cargo build --profile
measure`. The `fast` profile is `measure` with the Slint interface
unoptimized, which cuts a rebuild after a `.slint` edit from two minutes to
about 35 seconds. Use it for tests, local runs and checks: `cargo build
--profile fast` or `cargo test --profile fast`.

### Dependency sources

Yapfast patches these crates in `vendor/`, through `[patch.crates-io]` or a path
dependency in Cargo.toml:

- `i-slint-core`: Slint's core 1.18.1 with selectable `StyledText`,
  mentions coloured in a `TextInput`, timelines that keep their place as
  older messages load, a wheel that speeds up when spun fast, and middle-click
  autoscroll, above.
- `i-slint-renderer-skia`: Slint's Skia renderer 1.18.1 with slight hinting,
  mipmapped pictures, uncopied fonts and pictures, and Skia's own FreeType on
  Linux without wgpu, above, and a whole frame after every resize: Windows
  restores a minimized window at its old size without its pixels, which
  partial rendering left black.
- `matrix-rtc-livekit`: Element's matrix-rust-rtc at `5831af8`, with native
  audio tracks and cancellable joins.
- `webrtc-sys`: webrtc-sys 0.3.35 with a Windows link fix, capture leveling,
  remote track volume and camera capture from pooled buffers.
- `davey`: davey 0.1.4, keeping its key packages when a Go Live stream's group
  id changes, and reading a frame's overlong nonce without panicking.
- `tungstenite`: tungstenite 0.26.2, giving back the read buffer a large frame
  such as Discord's READY grew.
- `gpui-pre-windows`: a Windows build fix for the archived GPUI client. Nothing
  uses it now.

`vendor/x264-src` isn't a patched crate but x264's own C source, unchanged and
trimmed to what an 8-bit encoder library uses, with a build script in place of
its configure and Makefile. Go Live and the camera encode H264 with it; NASM
assembles its x86-64 code, so every shell, package and native build needs
`nasm`.

`vendor/ffmpeg-src` is FFmpeg's source trimmed to its hardware encoders
(NVENC, AMF on Windows, VA-API on Linux) and its HEVC decoder, with
nv-codec-headers', AMF's and libva's headers and configure's output for
Linux and Windows, all made by its `update.sh`, which configures Windows
with clang-cl against the MSVC runtime and SDK cargo-xwin downloads; three
patches let NVENC, AMF and VA-API change their bitrate without a keyframe.
Its build script compiles it with every symbol prefixed, apart from the
FFmpeg in libwebrtc, and loads libva, NVIDIA's driver and AMD's AMF runtime
only when an encoder opens. Its `README.yapfast.md`
explains the rest. Targets without a `gen/` folder build without hardware
encoding or H265 decoding.

Each has a `README.yapfast.md` with its changes, except the two Slint crates, which
this section describes. The Slint crates keep their `.rs` files at the crate
root, with no `src/`, both in `vendor/` and in Cargo's registry.

Crates that aren't vendored are in
`~/.cargo/registry/src/index.crates.io-*/NAME-VERSION/`. The Matrix SDK is a git
dependency; its checkout is `~/.cargo/git/checkouts/matrix-rust-sdk-*/c6ad52e/`,
the short form of the revision pinned in Cargo.toml. libDF's is
`~/.cargo/git/checkouts/deepfilternet-*/d375b2d/libDF/`. libwebrtc's headers are in
`$LK_CUSTOM_WEBRTC/include` inside the development shell.

## Checks

`scripts/check` enters the development shell itself. `scripts/check --fast`
builds with the `fast` profile, as local work does. It runs, in order:
formatting; `scripts/check-doc-links`, which fails on a Markdown link to a
missing file or heading; Clippy with warnings denied and the tests for the whole workspace
(Clippy leaves out the UI crate's own targets, which hold no code to lint, but
still checks it as a dependency); a build; and the desktop smoke test, which
runs `target/<profile>/yapfast`, the desktop build, from that build. After
those, one at a time, as each compile of the UI crate takes several GiB of
memory, it runs Clippy for the Discord-only and Matrix-only builds, then the
headless build's Clippy and smoke test in `target/check-combinations/`, so the
headless binary doesn't replace the desktop one. The Discord-only and
Matrix-only steps also select `crates/feature-union`, a crate without code
that turns on the features the full build turns on in the crates below Slint.
Slint and the UI crate then resolve as in the first Clippy step, and those
steps reuse its check of the UI crate, taking seconds instead of several
minutes each. When they compile `yapfast-ui` again, a dependency has drifted
and `feature-union` needs its features. Each step prints `=== step` and `rc=N` when it ends,
with Rust's errors when it fails, and keeps its full log under `target/check/`.
The script runs every step and exits non-zero if any failed.

A new worktree starts with an empty `target/`, and its first check compiles
every dependency. `scripts/warm-worktree`, run inside it
before the first build, copies the main checkout's `target/fast/` and
`target/check-combinations/` as reflinks, which share disk space with the
originals, and cleans the crates whose sources are in the repository, so those
build from the worktree's own files. The first check then takes about 3.5
minutes. Reflinks need a filesystem such as btrfs.

`git config core.hooksPath .githooks` turns on the pre-commit hook, which
checks formatting only, so commits stay quick.

## Seeing the app

`scripts/ui-session` runs Yapfast on a private Weston display, for screenshots and
checks by hand, without touching the desktop or a real account:

```sh
scripts/ui-session start      # the preview; --discord for the fake Discord
scripts/ui-session do 'click 40 100' 'type hello' 'key enter' 'shot sent'
scripts/ui-session relaunch   # rebuild and restart Yapfast on the same display
scripts/ui-session stop
```

Screenshots go to `.local/ui-session/shots/`, and `scripts/ui-session --help`
lists every option and command. `start --tray` adds a stand-in tray on the
session's bus; see [the tray](tray.md#testing). The scripted probes are in
[the Slint client's validation notes](slint-client.md#validation-and-limits).

For UI work, `start --live` builds with the `live-preview` feature. Slint then
generates a wrapper around its interpreter instead of compiled code: the window
loads `ui/*.slint` at startup and reloads them under a second after each save,
keeping its state. An edit that took a two-minute rebuild of
`crates/yapfast-ui` shows without one. Slint's watcher misses a save that
renames a new file over the old one, as `sed -i`, Claude's Edit tool and some
editors' atomic saves do; `touch` the file after such a save. Rust
changes, and properties or callbacks that Rust code starts using, still need
`relaunch`. The interpreter is slower and bigger than compiled code, so
measurements and footage of motion come from a normal build.

For demo footage, the session records its display at 60 frames a second:

```sh
scripts/ui-session start --discord --size 1920x1080 --zoom 1.2 --cursor 32
scripts/ui-session do 'record start tour' 'glide 400 600' 'click 170 264 glide' \
  'glide 1000 500' 'scroll -1200 1.5' 'sleep 1' 'record stop'
```

`testing/record_host.rs` takes the frames through Weston's capture protocol
and ffmpeg encodes them to `.local/ui-session/videos/tour.mp4`, which later
starts keep. `--zoom` opens Yapfast at that zoom level, `--cursor` shows the
pointer, `glide` and `click … glide` move it as a hand would, and `scroll
PIXELS SECONDS` scrolls smoothly. Weston's headless display repaints about
58.5 times a second at most, so about one frame in 40 repeats the one
before.

## Networks

Matrix and Discord are Cargo features, `matrix` and `discord`, and the default
build has both. A build can leave either one out:

```sh
cargo build --release --no-default-features --features desktop,discord  # Discord only
cargo build --release --no-default-features --features desktop,matrix   # Matrix only
```

A Discord-only build signs in by linking Discord by QR code, and connects with
the account linked on the device from then on. Voice calls
still need LiveKit's libwebrtc, which provides the audio processing and the
device list for both networks. The `--no-default-features` build without either
network only opens `--preview`.

`YAPFAST_DISCORD_TEST_SERVER=http://127.0.0.1:18400` points Discord at
`testing/fake_discord.ts` (`bun testing/fake_discord.ts --port 18400`). It uses a
test token instead of the linked account, so the real token never reaches it.
`scripts/test-fake-discord [FILTER]` starts the fake on a free port, waits until
it answers, runs the ignored tests in `crates/yapfast-discord/tests/fake_server.rs`
with the `fast` profile, and stops it. It then runs
`crates/yapfast-discord/tests/fake_env.rs` against a second fake that runs the
example environment.

`--env PATH` replaces the fake's built-in content with an environment, for
demos, promo recordings and screenshots. An environment is a JSON file, or a
TypeScript module whose default export is the environment. It sets up the
account, people with avatar pictures, statuses and friendships, DMs and
group DMs, and servers. A server has roles, members, categories, text
channels with history and unread counts, and voice channels with people in
them. One of those people can stream a video file with Go Live.
`testing/fake_discord_env.ts` defines the format as TypeScript types, and
`testing/envs/example.json` uses every part of it:

```sh
scripts/ui-session start --discord --fake-args '--env testing/envs/example.json'
```

Media paths are relative to the environment file. Videos need `ffprobe` and
`ffmpeg`. A streamed video is converted once to the fake's stream formats,
and the result is cached in the system's temporary directory. Without
`--env`, the fake serves its built-in content unchanged, and the tests rely
on it.
`YAPFAST_DISCORD_TEST_UNLINKED=1` starts Yapfast unlinked, to link against the fake.
[Network memory](measurements/README.md) compares the three builds.

With [direnv](https://direnv.net/), the repository's `.envrc` loads the Nix
development shell when you enter the folder, so commands need no
`nix develop` prefix. On NixOS, `programs.direnv.enable = true;` installs direnv
with nix-direnv, which caches the shell.

## Windows

The Linux cross-build and portable ZIP route is documented in the
[Windows beta guide](windows-beta.md). It supplies the matching Windows WebRTC
archive and checksum-pinned Skia source through the Windows Nix shell.
`scripts/build-windows` prepares and caches a static-runtime Skia build before
compiling Yapfast. The executable uses static CRT linkage to match LiveKit.
`scripts/build-windows [discord|full] [options]` builds the Discord-only release,
the default, or the full preview with Matrix, and passes the options to
`scripts/package-windows.py`, such as `--no-installer`.
`scripts/package-windows.py`, which `scripts/build-windows` runs, makes the
flavour's portable ZIP and compiles the installer, `packaging/windows/yapfast.iss`, with
Inno Setup 7.1.0 under Wine. The first run downloads Inno Setup's installer,
pinned by SHA-256, and installs it into its own Wine prefix in
`.local/windows/inno-setup`, under `xvfb-run`, since it opens windows even
when silent. `--no-installer` makes only the ZIP.

Binaries people download name no folder of the machine that built them.
Rust embeds source paths for panic messages, and C's `__FILE__` reaches
AWS-LC's errors and asserts, so a plain build names its builder's home folder
thousands of times, through `~/.cargo`. `scripts/release-env TARGET`, sourced
before `cargo build --target TARGET`, maps the home folder, Cargo's folder and
the repository to `/home`, `/cargo` and `/yapfast` with `--remap-path-prefix`
and Clang's `-ffile-prefix-map`. On Windows it takes `cargo xwin env`'s
environment and adds the C flags to it, since `cargo xwin build` sets the
`CFLAGS_<target>` the `cc` crate reads first; `scripts/build-windows` uses it.
`scripts/package-windows.py` refuses an executable holding the home folder's
path in UTF-8 or UTF-16. A Linux build from the dev shell needs the same, and
its linker also writes a RUNPATH of `$PWD/outputs/out/lib` and Nix store
folders into the binary; the [AppImage](#linux-appimage) avoids both by
building in the Nix sandbox.
GPUI's FXC shader build is no longer used.

The supported build route for this migration is the Linux MSVC cross-build.
Native Windows development needs Rustup, Visual Studio C++ tools and SDK, CMake,
LLVM, Protobuf and NASM, plus the same prepared Skia archive through
`SKIA_BINARIES_URL`. Opus comes from the libopus in libwebrtc's archive.
That native-host preparation/build route has not been checked.
The downloadable package needs none of these tools to run.

Before accepting the Windows build, check text entry, selection, copy/paste,
keyboard focus, navigation, both renderers, resizing, normal close, and reopen
on the tester's actual GPU.
Windows execution has not been demonstrated by a Linux build.

## Linux AppImage

`scripts/build-appimage` makes the Linux release in `target/dist/linux`:
`Yapfast-VERSION-x86_64.AppImage`, its `.zsync` file, the source snapshot and
`SHA256SUMS.txt`. All four go on the release page. AppImageUpdate, and AppImage
managers such as Gear Lever, find a newer release through the `.zsync` file,
named by the update information in the runtime:
`gh-releases-zsync|yapfast|yapfast|latest|Yapfast-*-x86_64.AppImage.zsync`.
The first build compiles every dependency in the Nix sandbox; packing takes
about 10 seconds after that.

Nix builds the binary, `nix/package.nix`'s Discord-only release build, and
`nix/appimage.nix` puts it in an AppDir with the desktop file, the icons and
the AppStream metainfo, `packaging/linux/io.github.yapfast.Yapfast.metainfo.xml`.
It sets the binary's interpreter to `/lib64/ld-linux-x86-64.so.2` and removes
Nix's RUNPATH, so the binary runs on the host's loader, glibc and libstdc++, as
one built on a distribution would. That's what lets the host's GPU driver,
Mesa or NVIDIA's, load into it: a driver needs the glibc it was built for, and
NVIDIA's must match the kernel module, so neither can come in the AppImage.
Nothing else is bundled either. Besides the C and C++ runtimes, the binary
links only GLib, PulseAudio's client, fontconfig and FreeType, which every
desktop has, and it loads EGL, Vulkan, Wayland, X11 and PipeWire at runtime.

The derivation fails if the binary links anything else, or asks for a glibc
newer than 2.39 or a libstdc++ newer than GCC 14's. That covers Ubuntu 24.04,
Fedora 40, Debian 13 and anything newer. nixpkgs' glibc is newer than that, so
a nixpkgs update can raise what the binary asks for. Raise the floors in
`nix/appimage.nix` only knowing which distributions that drops. Older ones,
such as Ubuntu 22.04, would need a build against their own glibc.

`scripts/package-linux.py`, which runs in the development shell, with
squashfs-tools and zsync added from the shell's nixpkgs, adds the
licenses and notices, `THIRD-PARTY.json`, `SOURCE.txt` and `BUILD.json` to
`usr/share/doc/yapfast`, as the Windows package has them, through the helpers
in `scripts/release_package.py` that both packaging scripts use. Skia's
prebuilt Linux archive carries only Skia's own license, so the notices of its
third-party code come from the pinned Skia source,
`nix build path:./nix#skia-source`. Then it packs the AppDir with mksquashfs
(zstd level 19, 1 MiB blocks, fixed owners and times, so the same AppDir packs
to the same file) behind AppImage's official type-2 runtime, pinned by hash.
The runtime is static: it mounts the image with FUSE 3's `fusermount3` and
needs no libfuse, and `--appimage-extract-and-run` runs it without FUSE at all.
The flake sees only the files Git tracks, so add a new file under `nix/` or
`packaging/` before building.

It was checked in containers, on `scripts/ui-session`'s private display with
the fake Discord:

- Ubuntu 24.04 with Mesa, on Wayland and on X11 (a rootful Xwayland), with
  both renderers.
- Ubuntu 24.04 with NVIDIA's 580.119.02 driver, on an RTX 3090, on Wayland and
  X11 with the GPU renderer.
- Fedora 42 with Mesa, on Wayland with the GPU renderer, without FUSE.

Each run loaded that distribution's loader, glibc, libstdc++, EGL and display
libraries, and drew the window. Mesa had no driver for the NVIDIA card, so in
the Mesa containers the GPU renderer drew on llvmpipe. In Ubuntu, voice
played Bob's tone and sent speech played into the microphone, through the
distribution's libpulse; Bob's stream played; text typed on X11 went out; and
the tray icon, its menu and reopening from it worked on the session's bus.
Screen sharing hasn't been tried from the AppImage.

## Arch Linux packages

`packaging/arch/PKGBUILD` builds the Discord-only release from source with
Arch's toolchain, as `nix/package.nix` builds it: `cd packaging/arch && makepkg
-si`. It clones the tag of its `pkgver`, which `scripts/set-version` sets to a
stable version, so run `scripts/set-version 0.2.0` before committing and
tagging `v0.2.0`. The tag's commit holds the PKGBUILD, which can't hold its own
checksum, so that source has none; the libwebrtc and Skia archives keep theirs.
Arch's `llvm21` is a later 21.x than Nix's, which is why
`nix/prepare-skia-native.py` takes any LLVM 21. Nothing builds this PKGBUILD
automatically: build it in a clean `archlinux:base-devel` container before a
release, about 6 minutes from cold.

The releases carry a package of their own binary instead. `scripts/package-arch`,
run after `scripts/build-appimage`, packs the AppImage's binary, desktop file,
icon, metainfo and notices into `yapfast-VERSION-1-x86_64.pkg.tar.zst` beside
it, through `packaging/arch/release.PKGBUILD`, and adds it to
`SHA256SUMS.txt`. It's the AppImage's binary, which runs on the host's loader
and libraries, so the two packages can't differ. The script runs makepkg in an
`archlinux:base-devel` container with Docker, then installs the package there
and runs `yapfast --smoke-test`, in about 20 seconds. A nightly's version
becomes `0.2.0nightly.20261020.5`, which pacman sorts after 0.1.0 and before
0.2.0, as SemVer sorts the nightly; with a dot before `nightly`, pacman would
sort it after 0.2.0. `sudo pacman -U` installs the file or its link; there's no
repository, so `pacman -Syu` doesn't update it.

It was checked in an Arch container, on `scripts/ui-session`'s private display
with the fake Discord: both renderers drew the window, the GPU one through
Arch's EGL and Mesa (llvmpipe).

## Nightly releases

Releases are built locally, but a nightly of `main` builds on GitHub: Actions,
Nightly, Run workflow. `.github/workflows/nightly.yml` names it after the next
minor version, the day (UTC) and the workflow's run number, such as
`v0.2.0-nightly.20261020.5`, and titles it as the stable releases are, with the
commit: `Yapfast 0.2.0-nightly.20261020.5 (396abe2)`. The run number puts two
nightlies of one day in order, as a commit hash wouldn't, and gives a second
run on one commit a tag of its own. `scripts/set-version` writes that version
into `Cargo.toml` and `Cargo.lock` before building, so the app reports it and
the packages and `SOURCE.txt` name it. One job runs `scripts/build-appimage`
and `scripts/package-arch`, and another `scripts/build-windows` in the Windows shell, with the Windows Skia
build, cargo-xwin's MSVC files and the compiled Rust dependencies
(`Swatinem/rust-cache`) cached between runs. The Linux build has no cache: it
compiles in the Nix sandbox, which can't reach one. The last job publishes the
AppImage, its `.zsync` file, the Arch package, the Windows ZIP, the source snapshot and
`SHA256SUMS.txt` as a prerelease on that commit. Wine doesn't start on the
runner, so there's no installer. The release's notes list the commits since the
previous nightly, or the latest release, each with its author's GitHub account:
features and fixes by their Conventional Commit type, the rest folded away
(`.github/nightly-notes.jq`). Every nightly stays on the releases page. Both
builds zip the source snapshot with the commit's time on every file, so they
make the same file, and the release carries one. A prerelease is never
`latest`, so the AppImage's update information leaves stable installs on stable
releases, and takes a nightly AppImage to the latest stable release, not the
next nightly.

## The app icon

`packaging/icon.svg` is Yapfast's icon: the logo in marigold, its dots in
on-accent ink, so it reads on a light taskbar as well as a dark one. The UI
keeps drawing `ui/icons/logo.svg`.

On Linux the window's app ID is `io.github.yapfast.Yapfast` (`src/icon.rs`).
Wayland desktops show a window's icon only through the desktop entry of that
name, `packaging/linux/io.github.yapfast.Yapfast.desktop`;
`scripts/install-desktop-entry [EXECUTABLE]` installs it and the icon for
this user, running `target/release/yapfast` unless told otherwise. The
AppImage installs nothing, so Yapfast run from one writes the entry and the
icon to `~/.local/share` itself before it opens its window
(`src/desktop_entry.rs`), with Exec and TryExec naming the AppImage. It
rewrites them when the AppImage has moved, and leaves them alone when a package
has its own entry in `XDG_DATA_DIRS`, or an AppImage manager such as Gear Lever
made one that runs the same file. On X11 the window also carries the icon
itself. Notifications name the entry, so the notification server shows the
same name and icon.

On Windows, `build.rs` compiles `packaging/windows/yapfast.rc` with `llvm-rc`
and links the icon into `yapfast.exe`, for Explorer and shortcuts; the window
loads it from there for its title bar and the taskbar, each at the size
Windows draws it. After changing `icon.svg`, run `scripts/make-windows-icon`
to draw `yapfast.ico` again.

## Logs

Yapfast logs only its own operational events at info level and above. Dependency
logs are disabled on purpose, because they can expose authentication data or
message content. Yapfast ignores `RUST_LOG`; `YAPFAST_LOG=debug` turns on Yapfast's own
debug logs (`crates/yapfast-core/src/logging.rs`).
Typed text never enters the log. Future errors must use fixed event
names and safe classifications rather than raw protocol responses or SDK Debug
output.

## Initial resource measurement

After a `measure` build, open `target/measure/yapfast` inside the Nix shell; measure
`release` only for the shipped build's absolute numbers. Leave it inactive for
at least 15 seconds, then sample its PID on Linux. `scripts/ui-session` writes
the PID of the Yapfast it runs to `.local/ui-session/yapfast.pid`.

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
Yapfast's native experiment sets the local authorization fallback explicitly,
because this stack does not advertise a production RTC discovery endpoint.
See [sessions](sessions.md) for credential storage and headless sign-in.

The service only allows the local `synapse` homeserver to create SFU rooms. This
does not establish a production friend-group or room-membership access policy.
That policy remains part of the encrypted calling experiment.
