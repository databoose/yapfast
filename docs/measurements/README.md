# Resource measurements

## Vesktop 1.6.7 and Yapfast 0.2.0

| | Vesktop memory | Yapfast memory | Vesktop CPU | Yapfast CPU |
| --- | --- | --- | --- | --- |
| Idle | 724 MB | 30 MB (−96%) | 0.11% | 0.015% (−86%) |
| Image-heavy channel | 706 MB | 41 MB (−94%) | 0.17% | 0.008% (−95%) |
| In a call | 888 MB | 44 MB (−95%) | 0.46% | 0.33% (−28%) |
| Sharing your screen¹ | 1,013 MB | 145 MB (−86%) | 2.15% | 2.9% (+35%) |

Vesktop is basically Discord's own app: the same Discord web app, in the same
Electron, with Vencord on top. So its numbers stand in for Discord's.

¹ Sharing a 1920×1080 monitor at 30 frames a second, while in the call.
Yapfast uses more CPU here than Vesktop, as it converts and encodes each frame
on the processor.

<details>
<summary>How these were measured</summary>

Measured on 25 October 2026 on the same PC: an AMD Ryzen 9 9900X (12 cores,
24 threads) with 32 GB of RAM, under NixOS with Hyprland, and each app's
default renderer (the GPU for Vesktop, the CPU for Yapfast). Yapfast is the
0.2.0 Linux release binary.

Memory is what Windows' Task Manager counts: each app's own memory across all
its processes, without the program files and fonts it maps from disk. On
Linux, that's the private anonymous memory of every process in the app, plus
Yapfast's window frames, which Yapfast draws itself. CPU counts the whole
processor as 100%. The percentages compare Yapfast with Vesktop.

Vesktop's window was maximized at 1920×1044. Yapfast's was 1920×1080 on the
test display, and 941×1020 while sharing on the desktop.

- Idle: a DM open, a minute or two after starting.
- Image-heavy channel: scrolled up through about a hundred pictures, then back
  down to the newest.
- In a call: measured from the call's third minute to its fifth, with the
  microphone on and a text channel open. Yapfast's memory held steady from
  the second minute to the sixteenth.
- Sharing your screen: the call, live with one monitor, measured from the
  stream's second minute to its third. Yapfast captures and encodes in a
  second process that runs only while the stream is live; it takes about
  105 MB of the 145 MB.

Vesktop ran on a real account, which has more servers than Yapfast's, once
for each scenario; its call had one friend talking now and then. Yapfast ran
against the local test server (`testing/fake_discord.ts`), five times for each
scenario, which stayed within 1 MB of each other, and twice for screen
sharing; its call had three people talking without pause. Yapfast shared a
real screen on the Hyprland desktop through the screen sharing portal.

</details>

## Dated records

This file holds dated measurement records, oldest first. A record describes
the code on its date; a later record of the same thing replaces it. Each says
how to repeat it; the raw samples behind them aren't published.

The current references are the newest:
[Matrix, Discord or both](#2026-10-06-matrix-discord-or-both) for idle memory
by build, and [where the memory goes](#2026-10-05-where-the-memory-goes-renderer-and-allocator)
for a call with Skia Software, the default renderer.
[Startup payload and preview copies](#2026-10-09-startup-payload-and-preview-copies)
records the first memory pass on a larger fake Discord account, and
[viewport media and lazy avatars](#2026-10-09-viewport-media-and-lazy-avatars)
the pass after it.

Measurements use `measure` builds, as the owner decided on 2026-10-06, unless the
question is the shipped build's absolute number; then use `release`. The
2026-10-05 profile comparison found the
two equivalent. Until 2026-10-17 the `measure` profile was named `fast`; the
`fast` profile now leaves the Slint interface unoptimized, so don't measure
with it.

How to reproduce: run every command from the repository root. The isolated
runs need `scripts/test-stack up` first; `nix-shell --impure
testing/display/shell.nix` adds Weston, PulseAudio and the ALSA plugins
to the development shell, and `scripts/run-framework-isolated` gives the run a
private display, D-Bus and audio. The scripts measure `target/release/yapfast`
unless `--exe` names another build, such as `target/measure/yapfast`; the networks
comparison takes its builds from a folder instead. `TEST_BIN`
below is the `yapfast-core` test binary that runs the other participants and
seeds history. Build it with `nix develop path:./nix --command cargo test
--profile fast --locked -p yapfast-core --no-run --message-format short`, which
prints its path on the `Executable unittests src/lib.rs
(target/fast/deps/yapfast_core-HASH)` line.

`scripts/measure-process` samples once a second and reports the mean and the
highest of those samples. For the process's lifetime peak, read `VmHWM` from
`/proc/PID/status`.

## 2026-10-01: Linux starter baseline

This record is
historical: it measured the GPUI starter, which Slint has replaced. It measures
the empty release build-check window after 15 seconds of warm-up, inactive at a
760x560 window size. It samples the process for 30 seconds through `/proc`.

| Metric | Result |
| --- | --- |
| Mean and peak RSS | 206.6 MiB |
| Mean PSS | 157.8 MiB |
| CPU, one core as 100% | 0.133% |
| GPU framebuffer memory, NVIDIA pmon | 44 MB |
| GPU utilization | Unavailable |

The machine has a 12-core AMD desktop CPU, six logical CPUs available, 31 GiB RAM,
and a desktop NVIDIA GPU. The stripped binary is 48.6 MiB.
The app runs two Tokio workers and the native GPUI renderer. No account, Matrix
sync, crypto store, room timeline, audio capture, or call is active.

These measurements establish the starter's cost on the development host. They
do not verify the beta's idle or ten-person-call targets, behavior on older
integrated graphics, battery use, or savings compared with Discord. GPU counters
were unavailable, so this check also does not establish inactive rendering cost.

Reproduce: the GPUI build is gone; its process was sampled with
`scripts/measure-process PID --seconds 30`. Keep hardware, workload, warm-up,
window activity, and memory metric attached to every result.

## 2026-10-02: Native voice audio

This record is a
functional two-process virtual-device result, rather than a performance sample.
It checks encrypted capture/playback after a 30-second wait alone, both audio
directions, mute/deafen and clean departure. Physical listening and Windows
remain untested.

Reproduce: `scripts/test-stack up`, then `nix develop path:./nix --command
scripts/test-voice --fast` in a desktop session with PipeWire; it adds and
removes its own null devices.

## 2026-10-02: Waiting alone in voice, debug build

This record samples a real account with one muted participant after confirming encryption
and waiting at least 15 seconds. Mean RSS was 324.1 MiB, mean PSS 307.9 MiB,
and CPU 1.03% of one core over 30 seconds. GPU counters were unavailable.
This debug build uses line-table symbols. It cannot be compared directly with
the empty release starter above and does not establish the beta budgets,
battery behavior or game interference. Release measurements on the reference
Linux and Windows machines remain needed.

Reproduce: not exactly, since it used a real account. Sample a debug build
waiting in voice with `scripts/measure-process PID --seconds 30`.

## 2026-10-03: Connected release client

This record samples
the Slint release build with a real account on the local stack, on a private
headless Weston desktop with private PulseAudio null devices. Each profile
warms up for 30 seconds, then samples Yapfast's process for 60 seconds at 1120×760.
A separate load process runs Bob and eight more accounts with generated tones,
so only Yapfast's own memory is counted.

| Profile | GPU PSS | GPU RSS | GPU CPU | Software PSS | Software RSS | Software CPU |
| --- | --- | --- | --- | --- | --- | --- |
| Settled idle, encrypted channel open | 191.3 MiB | 252.8 MiB | 0.52% | 88.2 MiB | 112.0 MiB | 0.45% |
| One encrypted message every 2 s | 201.9 MiB | 263.0 MiB | 0.72% | 89.9 MiB | 113.6 MiB | 0.60% |
| Alone in voice, muted | 223.8 MiB | 286.3 MiB | 0.58% | 109.2 MiB | 134.6 MiB | 0.72% |
| Two people, Bob speaking | 226.1 MiB | 288.7 MiB | 1.58% | 111.3 MiB | 136.8 MiB | 1.37% |
| Ten people, everyone sending audio | 229.4 MiB | 291.9 MiB | 7.01% | 114.8 MiB | 140.3 MiB | 6.91% |

CPU is a percentage of one core. NVIDIA's monitor reported 35–36 MiB of GPU
framebuffer memory for the GPU renderer; utilization counters were unavailable.
The ten-person call is below the 400 MiB target with either renderer. Settled
idle CPU is below 1%. Idle memory meets the 250 MiB target by PSS with both
renderers, and by RSS with Skia Software. GPU rendering's RSS is 2.8 MiB over;
RSS counts shared graphics-driver libraries in full, while PSS divides them.
Idle CPU stayed at about 0.5%, so the window did not repaint continuously.

This is one Linux host with generated audio and a local homeserver. Windows,
older or integrated GPUs, a large account, physical devices and game
interference remain unmeasured. The client measured predates the Milestone 2
chat features.

Reproduce, with the test stack up and a release build: `nix-shell --impure testing/display/shell.nix --run 'scripts/run-framework-isolated --audio --no-services --alsa-plugins "$YAPFAST_FRAMEWORK_ALSA_PLUGINS" --compositor-log .local/measure.weston.log -- python3 testing/measure_connected.py --load-executable TEST_BIN --output .local/measure-connected'`.
It measures both renderers unless `--renderer` names one.

## 2026-10-04: Connected client after Milestone 2 and the UI overhaul

This record repeats the 2026-10-03 run on the same host, with the same workload, warm-up and
sampling. It uses the release build with the conversation features, the Satoshi
fonts and the Discord-style layout. The empty run keeps the baseline's empty
channel. The seeded run first posts the sample conversation: Markdown, an
800×500 image, a file and a reply (`testing/measure_connected.py --seed`).

| Profile | GPU PSS | GPU RSS | GPU CPU | Software PSS | Software RSS | Software CPU |
| --- | --- | --- | --- | --- | --- | --- |
| Settled idle, empty channel | 195.1 MiB (+3.8) | 256.3 MiB | 0.35% | 97.8 MiB (+9.6) | 119.2 MiB | 0.35% |
| Message every 2 s, empty channel | 204.9 MiB (+3.0) | 266.1 MiB | 0.68% | 99.9 MiB (+10.0) | 121.3 MiB | 1.47% |
| Alone in voice, muted | 229.4 MiB (+5.6) | 291.2 MiB | 0.48% | 119.5 MiB (+10.3) | 142.3 MiB | 0.53% |
| Two people, Bob speaking | 231.3 MiB (+5.2) | 293.7 MiB | 1.10% | 121.4 MiB (+10.1) | 144.3 MiB | 1.26% |
| Ten people, everyone sending audio | 234.9 MiB (+5.5) | 297.3 MiB | 6.15% | 127.2 MiB (+12.4) | 150.1 MiB | 5.44% |
| Settled idle, seeded history | 207.9 MiB | 269.6 MiB | 0.30% | 104.0 MiB | 125.4 MiB | 0.38% |
| Message every 2 s, seeded history | 216.3 MiB | 278.1 MiB | 0.75% | 105.4 MiB | 126.7 MiB | 0.67% |
| Ten people, seeded history | 241.9 MiB | 304.9 MiB | 6.07% | 131.7 MiB | 154.7 MiB | 6.02% |

Brackets give the PSS change from the 2026-10-03 baseline. CPU is a percentage of
one core. NVIDIA's monitor reported 36–37 MiB of GPU framebuffer memory.

The new features cost 3–6 MiB of PSS with GPU rendering and about 10–12 MiB
with Skia Software. Visible media adds about 13 MiB with GPU and 6 MiB with
Software. The cause of the increases was not profiled. Settled idle CPU stays
below 0.4%, and the call profiles use no more CPU than before. Receiving
messages in an empty channel took 1.47% with Software, against 0.60% before. In
the seeded channel it took 0.67%. While a channel's history is shorter than the
window, Yapfast anchors it to the bottom, so each new message moves every row.

The ten-person call stays below 400 MiB. Idle PSS meets the 250 MiB target with
both renderers, and so does Software RSS. GPU RSS is now 6.3 MiB over with an
empty channel and 19.6 MiB over with the seeded history; it was 2.8 MiB over
before. RSS counts shared graphics-driver libraries in full. The measurement
limits listed for the 2026-10-03 run still apply.

Reproduce: the 2026-10-03 command above; add `--seed` for the seeded rows.

## 2026-10-05: Joining a 20-person voice channel

This record measures joins with one process per person (`testing/join_voice.py`). Alice runs
the release Yapfast window, muted. The other 19 are separate release processes on
the same voice stack. Each one publishes a tone in turn, sends digital silence
with DTX the rest of the time, and decodes every stream it receives. People join
12 s apart, so each arrival finds a settled call and rotates the media key. At
full size one person leaves and rejoins three times. Yapfast, every participant,
Synapse and LiveKit share one host, so network latency is near zero.

| Per join | Two peer connections | One peer connection |
| --- | --- | --- |
| Join call returns, mean of 19 joins | 1.74 s | 0.61 s |
| Join call returns, rejoin at 20 people | 1.24–1.36 s | 0.13–0.24 s |
| Newcomer hears everyone, mean | 2.21 s | 1.73 s |
| Newcomer hears everyone, rejoin at 20 | 1.7–1.8 s | 1.4–1.6 s |
| Everyone hears the newcomer, rejoin at 20 | 1.5–1.6 s | 1.0–1.4 s |
| CPU time of the other 19 clients, rejoin at 20 | 1.8–1.9 s total | 1.8–1.9 s total |

Join time did not grow with call size in either configuration. An earlier run
had grown to about 19 s per join at 50 people, but that put every remote person
in one debug-build process, so the process, not the protocol, was the limit. The
cost that does grow is spread out. Each existing client spends about 0.1 s of
CPU per arrival, because a newcomer in a settled call makes everyone rotate and
redistribute their key.

The LiveKit connection took most of the join. With separate publisher and
subscriber connections, the subscriber connected about one second after the
publisher on every join. One peer connection removes that, and now the time to
hear everyone is mostly key delivery through Matrix sync. In the 20-person call,
the Yapfast window used 3.5% of one core and 243 MiB PSS (307 MiB RSS). Each
headless participant used 3.6% and 26 MiB PSS (98 MiB RSS) without any UI.
Steady state had no decryption failures and no participant who could not hear
another. The 197 missing-key events came from audio that arrived just before its
key during joins. Yapfast now shows a missing key only once it has lasted beyond the
10 s grace period.

On a real network every round trip adds latency, so absolute join times will be
longer than here. Windows, real devices and a remote homeserver remain unmeasured.

Reproduce, with the test stack up and a release build: `nix-shell --impure testing/display/shell.nix --run 'scripts/run-framework-isolated --audio --no-services --alsa-plugins "$YAPFAST_FRAMEWORK_ALSA_PLUGINS" --compositor-log .local/join.weston.log -- python3 testing/join_voice.py --participant-executable TEST_BIN --output .local/join-voice'`.
Current code gives the one-peer-connection column.

## 2026-10-05: A call with the window hidden or destroyed

This record measures what a tray could save (`testing/measure_background.py`). Yapfast has no
tray yet, so a test-only window control (`YAPFAST_UI_TEST_CONTROL`) hides the window,
destroys it and creates it again while Yapfast stays in the call. Alice runs the
release Yapfast window with GPU rendering, the sample conversation loaded, unmuted
in a ten-person call. The other nine are headless participant processes, as in
the join measurement. Each state settles for 30 s and is sampled for 60 s.

| Yapfast in a ten-person call | PSS | RSS | CPU, one core |
| --- | --- | --- | --- |
| Window open, voice view | 234 MiB | 305 MiB | 6.0% |
| Window hidden | 234 MiB | 304 MiB | 5.8% |
| Window destroyed | 235 MiB | 305 MiB | 5.5% |
| Window destroyed, after `malloc_trim` | 210 MiB | 280 MiB | 5.6% |
| Window created again | 235 MiB | 306 MiB | 5.9% |
| Headless participant, no UI | 25 MiB | 91 MiB | 2.5% |

Hiding the window saves nothing. Slint keeps the whole component, its models and
the renderer. Destroying it releases the component, but glibc keeps the freed
heap, so memory does not drop until `malloc_trim` returns about 25 MiB. Most of
Yapfast's memory is not tied to the window. Of the 234 MiB PSS with the window open,
93 MiB is resident code from Yapfast's own binary (Skia, WebRTC, Slint and the Matrix
SDK), 48 MiB is the GPU driver, and 86 MiB is heap and anonymous memory. Destroying
the window unmaps neither the code nor the driver. Yapfast's anonymous memory falls
from 98 MiB to 73 MiB after the trim. A headless participant with the same voice
stack has 15 MiB. Its RSS counts the shared test binary in full, and its CPU leaves
out audio devices and audio processing.

So a tray that hides the window keeps Yapfast's full cost. Destroying the window
and trimming the heap saves about a tenth. A small background process for the
session and call, with the window in a separate process started on demand,
would come close to the headless figures.

Reproduce, with the test stack up and a release build: `nix-shell --impure testing/display/shell.nix --run 'scripts/run-framework-isolated --audio --no-services --alsa-plugins "$YAPFAST_FRAMEWORK_ALSA_PLUGINS" --compositor-log .local/background.weston.log -- python3 testing/measure_background.py --participant-executable TEST_BIN --people 10 --output .local/measure-background'`.
It renders with Skia GPU unless given `--renderer software`.

## 2026-10-05: Where the memory goes: renderer and allocator

This record repeats the window measurement with glibc's allocator statistics (`malloc_stats`)
and a full smaps dump at each sample. It compares GPU rendering, GPU rendering
with at most two heap arenas, and Skia Software. A heaptrack snapshot of the
signed-in, synced client before it connected to the call found 28 MiB of
anonymous memory and at most 9.7 MiB of live heap. Under heaptrack the voice join
did not finish, so it could not profile the call itself.

| Ten-person call, window open | PSS | Anonymous | Heap in use | Heap held | Arenas | CPU, one core |
| --- | --- | --- | --- | --- | --- | --- |
| GPU, glibc defaults | 247 MiB | 98 MiB | 78 MiB | 91 MiB | 47 | 5.8% |
| GPU, `MALLOC_ARENA_MAX=2` | 226 MiB | 90 MiB | 78 MiB | 79 MiB | 2 | 5.6% |
| GPU, arena cap built in | 238 MiB | 90 MiB | 78 MiB | 79 MiB | 2 | 5.5% |
| Skia Software, glibc defaults | 131 MiB | 32 MiB | 18 MiB | 29 MiB | 43 | 5.6% |

The renderer decides most of Yapfast's private memory. With GPU rendering the heap
holds 78 MiB in use, against 18 MiB with Skia Software. The difference comes from
the graphics driver and Skia's GPU state, not from the chat or the call. Hiding
the window frees 24 MiB of that heap, but glibc keeps it, and it only goes back
to the system after `malloc_trim`. In this voice view both renderers used the
same CPU.

glibc gave Yapfast's threads 47 arenas, which held 13 MiB more than was in use.
Yapfast now caps them at two at startup unless `MALLOC_ARENA_MAX` is set. That
removed the gap and 8 MiB of anonymous memory at the same CPU. PSS varies by
about 10 MiB between runs of the same build, so the allocator columns are the
better comparison. Lock contention in real-time audio threads was not measured
beyond CPU and a clean call.

Reproduce: the window command above three times, as is, with
`--yapfast-env MALLOC_ARENA_MAX=2` and with `--renderer software`. The built-in cap
row is the first command on a build since the cap.

## 2026-10-05: Scrolling a long history with each renderer

This record checks whether Skia Software keeps up with the GPU when it has to redraw
(`testing/measure_scroll.py`). Bob posts the sample conversation and 300 more
messages. Yapfast opens the channel and settles, then the private seat sends 60
touchpad scroll events a second, changing direction every two seconds. Frame
rates count the buffers Yapfast submitted to the compositor, not frames presented.

| Release build | Idle PSS | Scrolling PSS | Scrolling CPU | Buffers/s |
| --- | --- | --- | --- | --- |
| GPU, 1120×760 | 214 MiB | 223 MiB | 5.0% | 39.9 |
| Software, 1120×760 | 98 MiB | 98 MiB | 3.0% | 39.8 |
| GPU, 3600×2000 at 2× | 219 MiB | 228 MiB | 5.4% | 39.9 |
| Software, 3600×2000 at 2× | 132 MiB | 131 MiB | 4.2% | 39.6 |

Skia Software used less CPU than the GPU at both sizes, and about 90 MiB less
memory even with the window's pixel buffers about eight times larger. Both
renderers submitted the same number of buffers. They stopped near 40 a second,
below the 60 scroll events, so the input path or Slint's frame pacing sets that
limit, not the renderer. Settled idle CPU stayed below 0.5% throughout. Yapfast now
renders with Skia Software by default; `--renderer gpu` opts into the GPU.

This harness found that scrolling up never left the newest message. When the
list measured newly visible rows, Yapfast snapped back to the end before it saw the
scroll. The timeline now checks the position before following new messages, and
`scrolling_up_leaves_the_latest_message` covers it.

Reproduce, with the test stack up and a release build: `nix-shell --impure testing/display/shell.nix --run 'YAPFAST_FRAMEWORK_SURFACE_LOG=.local/measure-scroll-surface.ndjson scripts/run-framework-isolated --no-services --compositor-log .local/scroll.weston.log -- python3 testing/measure_scroll.py --fixture-executable TEST_BIN --output .local/measure-scroll --size 1120x760 --scale 1'`,
then `python3 testing/measure_scroll.py --frames .local/measure-scroll/results.json
.local/measure-scroll-surface.ndjson` for the buffer rates. For the large rows,
give `scripts/run-framework-isolated` `--width 3840 --height 2160 --scale 2` and
the script `--size 3600x2000 --scale 2`.

## 2026-10-05: Release and fast builds

This record measures the same source built with the `release` profile (thin LTO) and the
`fast` profile (no LTO, incremental), back to back.

| Profile | Release | Fast |
| --- | --- | --- |
| Ten-person call, GPU, window open: PSS | 237–241 MiB | 232–236 MiB |
| Ten-person call, five window states: CPU | 5.7–6.4% | 5.8–6.6% |
| Ten-person call, window open: heap in use | 78 MiB | 78 MiB |
| Scrolling, Software, 1120×760: PSS | 104 MiB | 100 MiB |
| Scrolling, Software, 1120×760: CPU | 2.8% | 3.0% |

The differences are within the run-to-run variation of each build. Tests,
local checks and measurements use the `fast` profile; `release`, which is what
people run, is for the shipped build's absolute numbers.

Reproduce: build both profiles, then run the window command above and the
scroll command with `--exe target/release/yapfast` and with `--exe target/fast/yapfast`.

## 2026-10-06: Matrix, Discord or both

This record compares
builds with only Matrix, only Discord, or both, made with the `matrix` and
`discord` Cargo features ([building](../building.md#networks)). Both networks
run on test data: Matrix on the local stack with a seeded #general, Discord on
`testing/fake_discord.ts` with the same 60 messages, image, file and reply
(`testing/measure_networks.py`). The combined build is measured with each
network's #general open. Builds use the `fast` profile and Skia Software at
1120×760. Each sample is 30 s after a 20 s warm-up, with two runs in opposite
orders.

| Build, open channel | PSS idle | RSS idle | PSS, message every 2 s | CPU, message every 2 s | Binary |
| --- | --- | --- | --- | --- | --- |
| Discord only, Discord #general | 65.9–66.1 MiB | 90.5–90.8 MiB | 66.0–66.1 MiB | 0.5–0.6% | 101 MB |
| Both, Discord #general | 109.2–110.4 MiB | 134.0–135.0 MiB | 109.2–110.4 MiB | 0.9–1.0% | 241 MB |
| Matrix only, Matrix #general | 92.1–118.9 MiB | 117.0–143.5 MiB | 96.6–120.3 MiB | 1.2% | 227 MB |
| Both, Matrix #general | 94.8–116.2 MiB | 119.0–140.9 MiB | 100.7–120.7 MiB | 1.3–3.7% | 241 MB |

Binary sizes are unstripped `fast` builds. With Discord open, leaving Matrix
out saves 43–44 MiB of PSS, 40%, and both runs agree within 1.3 MiB. Most of
it is Matrix's sync, encrypted stores and room list running in the background,
not the open channel.

The Matrix rows vary by about 24 MiB between runs. Whichever Matrix profile ran
first measured higher, even after an unmeasured launch of the same account
beforehand, so the cause isn't the first restore. Measured second, the
Matrix-only and combined builds differ by 3 MiB, so Discord's connection costs
little next to Matrix. Repeated, interleaved runs would pin both down.

The test data is small on both sides: one server and 60 messages. A real
Discord account's READY snapshot, which Yapfast keeps, and a real Matrix account's
stores both grow with the account. Voice isn't measured, because the fake has
no voice server, and Discord calls also load libwebrtc's audio processing.

Reproduce: with the test stack up, build each feature set with `--profile fast`
(`--no-default-features --features desktop,matrix`, then `desktop,discord`,
then the default) and copy each `target/fast/yapfast` to `target/networks/` as
`yapfast-matrix`, `yapfast-discord` and `yapfast-combined`. Then `nix-shell --impure testing/display/shell.nix --run 'scripts/run-framework-isolated --audio --no-services --alsa-plugins "$YAPFAST_FRAMEWORK_ALSA_PLUGINS" --compositor-log .local/networks.weston.log -- python3 testing/measure_networks.py --output .local/measure-networks --load-executable TEST_BIN'`.
The script starts the fake Discord itself.

## 2026-10-09: Startup payload and preview copies

This record compares the Discord-only `fast` build before and after freeing its READY
payload and making UI preview copies on demand. The fake account has 100
servers, 300 DMs, 12 extra channels and 60 authors. Its #general has 60
messages, with an image every third extra message. Both builds use Skia
Software at 1120×760, the same private display and fake server, and the same
saved test account. Each view settles for four seconds and is sampled for
five seconds. Two runs of each build alternate in the order before, after,
after, before.

| View | RSS before | RSS after | PSS before | PSS after | Live heap before | Live heap after |
| --- | --- | --- | --- | --- | --- | --- |
| DM list and Bob's conversation | 207–212 MiB | 158–164 MiB | 180–185 MiB | 132–138 MiB | 99.5–99.6 MiB | 67.6–67.8 MiB |
| Image-heavy #general | 220–223 MiB | 170–176 MiB | 193–196 MiB | 139–150 MiB | 112.7–112.9 MiB | 74.9–75.0 MiB |

The server view saves 48 MiB of RSS on average, 22%, and 38 MiB of live
heap, 34%. Settled CPU is unchanged at 0–0.2% of one core. Captures of the
same server view differ by 0.022% normalized pixel RMSE.

The gateway previously cloned every JSON dispatch payload. The connection
task then borrowed READY for the entire session, keeping its original JSON
after the tracker had copied the fields it needed. It now moves dispatch
payloads and releases READY before waiting on the session. On glibc it calls
[`malloc_trim`](https://man7.org/linux/man-pages/man3/malloc_trim.3.html)
once at that point, returning freed startup pages to the OS. Without the
trim, much of the freed heap stayed resident.

The UI previously copied every loaded gallery image, sticker and embed image
into a Slint pixel buffer when building message rows, including rows outside
the viewport. An immutable
[Slint model](https://docs.slint.dev/latest/docs/rust/slint/trait.Model)
now copies pixels when the row is requested. Converted previews share an
eight MiB cache with least recently used eviction. Visible components keep
their image handles after eviction. Source dimensions, spoiler covers,
animation models, keyboard navigation and selection stay the same.

This investigation also inspected the owner's live process read-only. It
used 314 MiB RSS, 291 MiB PSS and 224 MiB anonymous memory with software
rendering and 13 threads. It received no input and was not restarted, so
the test account's saving is not a measured saving for that account. This
comparison has no active call or Matrix session and does not measure Windows.

The remaining media cost is in the backend's decoded previews for all loaded
history and recent channels. Loading those by viewport, with a bounded decoded
cache, would remove another copy for older images. Skia also copies visible
Slint pixels into its own raster images. Sharing those pixels needs an owned
buffer API with safe lifetimes; simply borrowing the bytes would be unsafe.
These opportunities are unimplemented and their savings are unmeasured.

A follow-up font-manager experiment
replaced Skia's system font manager with its
[`custom_empty` manager](https://github.com/rust-skia/rust-skia/blob/master/skia-safe/src/core/font_mgr.rs),
which loads the font data Fontique selected without another system font scan.
It ran the same fake account and display in the order before, experiment,
before, experiment. In the matched second server run, live heap fell by only
0.50 MiB, or 0.45 MiB after a diagnostic heap trim. RSS did not improve.
The matched captures differed by 0.000425% normalized pixel RMSE. The patch
was discarded. Allocator retention varied enough to outweigh the small
allocation saving, and the first server view also had an unread banner that
disappeared on later opens.

The next changes worth measuring are:

- Decode previews near the timeline viewport and release distant ones. The
  backend currently requests every preview in loaded history. One hundred
  360×270 RGBA previews take 37 MiB before UI or renderer copies.
- Give recent channels one byte budget for decoded previews. Forty closed
  channels can keep four previews each, about 59 MiB at maximum dimensions.
  The application and backend share these pictures, so that estimate counts
  each decoded picture once. The disk cache can supply evicted previews on
  reopening.
- Request and convert avatars by viewport. The member panel creates visible
  components through a `ListView`, but the application requests avatars for
  the whole member list and the UI converts all of them. Each 128×128 avatar
  costs 64 KiB in the core and another 64 KiB in Slint before renderer storage.
- Release idle animation decoders on a timer. Their cleanup currently runs
  only when another animation requests a frame; if the last visible animation
  scrolls away, its decoder and shown frame can stay allocated in its model.

These sizes describe allocations in the code, not measured savings on the
owner's account. The follow-up read-only process sample was 229 MiB RSS,
215 MiB PSS and 138 MiB anonymous memory. It received no input. The isolated
test session was stopped and the previously optimized binary restored.

Reproduce: save a Discord-only build of commit `fa88771` and one with these
changes, both built with `nix develop path:./nix --command cargo build --locked
--profile fast --no-default-features --features desktop,discord`. Start each
through `scripts/ui-session start --discord --fake-args '--servers 100 --dms
300 --images 3 --channels 12 --authors 60' --bin PATH`. For each run, open
DMs with `scripts/ui-session do 'click 36 36'`, wait four seconds, and sample
the PID in `.local/ui-session/yapfast.pid` with `scripts/measure-process PID
--seconds 5`. Repeat after opening the server with `'click 36 102'`. For
allocator totals, set `YAPFAST_UI_TEST_CONTROL` to a writable command file before
starting the private session and append `stats` after each sample. Use a fresh
command file on each restart, and retain the same fake server and test data
between runs with `scripts/ui-session relaunch`. The JSON records the build
hashes, individual samples and allocator totals.

Validation passed `scripts/check --fast`, including formatting, documentation
links, workspace tests, Clippy for each network combination, desktop and
headless builds and smoke checks. Regression tests cover moving the gateway
payload without copying it, lazy gallery allocation, cache bounds and valid
image handles after eviction. Existing UI tests cover animation decoding,
scrolling, selection, image copying and message actions.
The fake Discord gateway integration and paginated-history integration also
passed. Their runner returned 143 while its EXIT trap killed and waited for
the fake server; the tests themselves both reported success.


## 2026-10-09: viewport media and lazy avatars

The follow-up record compares commit
`fa3c205`, which includes the preceding startup fixes, with viewport-driven
media loading and shared decoded previews. It implements the four proposals
above. The Discord-only `fast` builds use the same private 1120×760 software
renderer display and fake account: 100 servers, 300 DMs, 12 extra channels,
60 authors, and three images in each generated gallery. There is no call.

Two runs of each build alternate before, after, after, before. Each view
settles for four seconds and is sampled for five seconds. The second pair
starts with an empty media disk cache; the saved account, fake server, display
and read markers are retained. The third optimized run checks the final
implementation after the message-ID viewport fix, and is excluded from the
paired averages. No sample uses a diagnostic allocator trim.

| View | RSS before | RSS after | Live heap before | Live heap after |
| --- | --- | --- | --- | --- |
| DM list and Bob's conversation | 158–168 MiB | 140–141 MiB | 67.6–68.1 MiB | 50.4–50.6 MiB |
| Image-heavy #general | 171–185 MiB | 145 MiB | 74.9–75.3 MiB | 52.6–52.7 MiB |
| After twelve other chats, reopening #general | 203–220 MiB | 165–167 MiB | 104.0–104.4 MiB | 69.5–70.0 MiB |

The paired server views save 33 MiB RSS on average, 19%, and 22 MiB live heap,
30%. After chat switching, they save 46 MiB RSS, 22%, and 34 MiB live heap,
33%. RSS varies more because glibc retains freed pages. Settled CPU samples
remain at 0–0.4% of one core. The matched second server captures differ by
0.028% normalized pixel RMSE. The final confirmation measures 145 MiB RSS in
#general and 166 MiB after chat switching.

With a cold disk cache, the initial server view requests 17 avatars and four
previews, versus 155 avatars and 20 previews. After twelve other chats, the
cumulative requests are 72 avatars and 40 previews, versus 215 and 260. The
fake's `/_control/media-requests` endpoint counts downloads; warm disk hits
bypass it.

The UI reports visible timeline items with six rows of margin on either side,
coalescing changes over 50 ms. Discord and Matrix decode previews for those
items and drop distant handles. Discord's recent channels share an 8 MiB LRU
budget for decoded RGBA pixels. Closing a channel drops preview handles from
both its backend and remembered UI timeline; cached pixels can be reused on
reopening, and evicted previews can be decoded again from disk. Visible rows
can hold images beyond the cache budget.

Virtualized server, DM, member, friend and message rows now carry avatar URLs.
They request and convert pictures as Slint instantiates nearby components.
Sources are weak pointers and converted avatars share the existing 8 MiB
Slint preview cache. Profile and call controls retain their direct loading.
A native regression with 500 servers and 500 members verifies that each list
requests only a few dozen nearby pictures and requests new ones on scrolling.
Core avatar retention still uses the existing application's retention policy.

Idle animation decoders now release their decoding buffers and last shown
frame on a one-second timer, including when no animation requests another
frame. The timer stops when it has no active decoders. A regression failed
with the previous opportunistic sweep and now passes using the testing clock.
Another regression covers a temporary Matrix message ID becoming its server
ID without changing the row count or scroll position.

Reproduce with Discord-only builds of `fa3c205` and this change. Build each
with `nix develop path:./nix --command cargo build --locked --profile fast
--no-default-features --features desktop,discord`, saving the binary outside
`target`. Start the baseline using `scripts/ui-session start --discord
--fake-args '--servers 100 --dms 300 --images 3 --channels 12 --authors 60'
--bin PATH`. Open DMs with `scripts/ui-session do 'click 36 36'`, then the
server with `'click 36 102'`. For chat switching, click sidebar rows at
`x=180`, `y=148+36*index` for index 1 through 12, waiting one second each,
and reopen #general at `(180,148)`. Move the pointer to `(550,25)` before
captures. Sample `.local/ui-session/yapfast.pid` with `scripts/measure-process
PID --seconds 5`; allocator totals use the isolated test-control file as in
the preceding record. Swap the saved binaries into `.local/ui-session/yapfast`
by replacing the file and use `scripts/ui-session relaunch` to keep the same
fake and display. Reset the fake's counters with `scripts/ui-session do
'control media-requests'` before each relaunch. For the cold pair, remove only
the disposable `.local/ui-session/data/discord-pictures` cache before relaunch.

Core, native UI and fake Discord integration regressions pass. The Matrix-only
integration covers encrypted image metadata before viewport requests,
preview decoding and release, forwarding and saved drafts against the local
disposable test stack. The owner's live Yapfast process received no UI input
and was not restarted. Memory savings on that account are unmeasured.

`scripts/check --fast` passed formatting, documentation links, Clippy for all
network combinations, workspace tests (49 application, 129 core and 105
protocol tests), desktop and headless builds and smoke checks. The private
UI session was stopped and the Discord-only `target/fast/yapfast` rebuilt for
the owner's next launch.

## 2026-10-13: Discord server foundations

The record compares commit
`bb26acf` with the server foundations: members' roles held once as shared
`Arc<[String]>` that member list rows and `ChatMember` point at, rather than
a second copy in each listed member; one shared notification settings
object per server; server pins and search; and menus with a submenu per row.
Discord-only `fast` builds ran on the busy fake account with the server's
#general and its member list open.

| Build | RSS, mean of 3 (range) | PSS, mean of 3 |
| --- | --- | --- |
| `bb26acf` | 108.2 MiB (106.6–110.6) | 88.9 MiB |
| This change | 106.6 MiB (105.0–107.0) | 87.0 MiB |

The difference is inside the spread between runs of one build, so the change
costs nothing measurable here. The roles it stops copying are a few hundred
bytes per listed member, at most 4,000 of them across the 40 kept lists.

Reproduce: build each with `nix develop path:./nix --command cargo build
--locked --profile fast --no-default-features --features desktop,discord`,
saving the binary outside `target`. For each run, start a fresh session with
`scripts/ui-session start --discord --bin PATH --fake-args '--servers 100
--dms 300 --images 3 --channels 12 --authors 60'`, then `scripts/ui-session
do 'sleep 8' 'click 36 36' 'sleep 4' 'click 36 102' 'sleep 8' 'move 550 25'
'sleep 4'`, and sample `.local/ui-session/yapfast.pid` with
`scripts/measure-process PID --seconds 10`. Alternate the builds.

## 2026-10-13: jumping to a message

The record compares commit `fa68b5f`
with `7c58d05`, whose jumps load the messages around the one jumped to, as a
window of older messages, rather than every page between the newest message
and it. The measured build lacked three fixes made after it, none on this
path: Matrix's live timeline keeping a window's first messages, history
answering when Discord is away, and no earlier page while a window loads.
Discord-only `fast` builds ran on the fake with `--extra 940`, which gives
#general 944 messages, and jumped from the pins to Bob's welcome, the
channel's first message.

| Build | Requests per jump | Heap in use, #general | Heap in use after the jump | Heap held after the jump | Peak RSS |
| --- | --- | --- | --- | --- | --- |
| `fa68b5f` | 16–17 pages `before` | 7.78 MiB | 13.92 MiB (13.76–14.21) | 17.29 MiB | 94.6 MiB |
| `7c58d05` | 1 `around`, 1 `before` | 7.78 MiB | 9.85 MiB (9.57–10.04) | 11.34 MiB | 89.0 MiB |

Means of three runs each. The jump grew the heap by 6.14 MiB before and by
2.07 MiB after: two thirds less, because the timeline now holds about 25
messages, and then what scrolling loads, rather than the 800 to 850 it
loaded getting there. Yapfast's whole heap is small, as the rest of its memory
is mapped code, fonts and Skia, so the peak RSS falls by less, 5.6 MiB. The
second request is the window's first page `before` its oldest message,
which comes back empty and shows the channel's start. Other worktrees' builds
ran meanwhile and the host was swapping, so the sampled RSS and PSS fell
whenever the kernel reclaimed Yapfast's file-backed pages; the record keeps
them, but heap in use, which reclaim doesn't change, is the comparison.

Reproduce: build each with `nix develop path:./nix --command cargo build
--locked --profile fast --no-default-features --features desktop,discord`,
saving the binary outside `target`. For each run, start a fresh session with
`scripts/ui-session start --discord --bin PATH --fake-args '--extra 940'`,
then `scripts/ui-session do 'sleep 8' 'click 36 102' 'sleep 8' 'move 550
25' 'sleep 4'`, sample `.local/ui-session/yapfast.pid` with
`scripts/measure-process PID --seconds 10`, and append `stats` to
`.local/ui-session/window-control`, which prints glibc's `malloc_stats` to
`.local/ui-session/yapfast.log`. Then `scripts/ui-session do 'control history'
'click 774 24' 'sleep 2' 'move 700 125' 'sleep 1' 'click 735 128' 'sleep 20'
'move 550 25' 'sleep 4'`, sample and print the statistics again, and read
the requests with `scripts/ui-session do 'get history'`. Alternate the builds.

## 2026-10-13: Discord server channel header

The record compares `fa68b5f`
with the server channel's header: each channel's topic held once as an
`Option<Arc<str>>` the server's snapshots share, rendered for the open
channel only and sent again only when it changes; the notification bell; and
the channel and category notification menus, whose rows are built when a
menu opens. Discord-only `fast` builds ran on the busy fake account with
#general, its topic and its member list open.

| Build | RSS, mean of 3 (range) | RSS peak, mean of 3 | PSS, mean of 3 |
| --- | --- | --- | --- |
| `fa68b5f` | 106.2 MiB (100.6–111.6) | 109.4 MiB | 85.4 MiB |
| This change | 105.1 MiB (101.4–107.8) | 107.1 MiB | 85.6 MiB |

The difference is inside the spread between runs of one build. A topic costs
each channel 16 bytes for its pointer, about 80 KiB across the fake's 5,000
channels, which have none, and its text once where Discord sends one. In the
second pair both builds released memory during the sample, so their peaks
are higher than their means. A first set, run while a build compiled beside
it, was discarded.

Reproduce as for the server foundations above, with the two builds.

## 2026-10-13: Matrix call cameras

This record measures Alice's `fast` build in a five-person call on the local stack as the
others turn their cameras on, then Alice, then everyone off, twice. The load
accounts film the 640×360 test pattern; each tile shows about 245×137 at
1120×760. The host was short of memory during the run: the kernel swapped part
of Yapfast out and dropped its library pages, so RSS and PSS fell as cameras
turned on. The table gives Yapfast's anonymous memory plus its swap, which those
don't move, and CPU as a percentage of one core.

| Profile | Software memory | Software CPU | GPU memory | GPU CPU | Threads |
| --- | --- | --- | --- | --- | --- |
| No cameras | 35.5 MiB | 4.9% | 101.3 MiB | 2.8% | 37 |
| One remote camera | 35.5 MiB | 11.3% | 101.3 MiB | 8.5% | 40 |
| Two remote cameras | 35.8 MiB | 10.3% | 101.3 MiB | 11.4% | 43 |
| Four remote cameras | 39.0 MiB | 18.4% | 104.1 MiB | 17.9% | 49 |
| Four remote cameras and Alice's own | 54.8 MiB | 20.1% | 120.8 MiB | 17.9% | 55 |
| Every camera off again | 55.0 MiB | 2.2% | 120.8 MiB | 5.3% | 54 |
| On and off a second time | 56.1 MiB | 2.9% | 121.8 MiB | 4.4% | 54 |

Threads are the Software run's. Each remote camera adds three: its decoder,
its incoming stream and its frame decryptor. Remote cameras cost little
memory, because each frame is decoded into WebRTC's own buffer and converted
straight into the tile's. Alice's own camera costs about 16 MiB, nearly all
of it LiveKit's VP8 simulcast encoder. CPU on this host was noisy: another
Yapfast session shared it.

Turning cameras off mutes the tracks and pauses the streams at the server, so
CPU returns to the call's own, but the encoder and the idle decoders stay
until the call ends. Unpublishing would free nothing: an earlier run that
unpublished Alice's camera grew by about 17 MiB and nine threads with each
round, because LiveKit leaves an unpublished track's sender and encoder in
the connection. Muting keeps the second round level. A later quick run with
the camera source's pool emptied and the heap trimmed on camera-off returned
3 MiB of it (53.4 to 50.4 MiB, Software).

Reproduce, with the test stack up: `nix-shell --impure testing/display/shell.nix --run 'scripts/run-framework-isolated --audio --no-services --alsa-plugins "$YAPFAST_FRAMEWORK_ALSA_PLUGINS" --compositor-log .local/measure.weston.log -- python3 testing/measure_cameras.py --load-executable TEST_BIN --output .local/measure-cameras'`.

## 2026-10-13: Discord call cameras

The record
measures the Discord-only `fast` build in a four-person call in Lounge on
`testing/fake_discord.ts`, through `scripts/ui-session`. Bob, Carol and Dave
film a 720p H264 recording at 30 frames a second and 1.5 Mbit/s, made with
ffmpeg's `testsrc2` (the command is in the script), and Alice the 640×360
test pattern. Their tiles are about 372×209 in a 2×2 grid at 1120×760.
Memory is anonymous memory plus swap; CPU is a percentage of one core.

| Profile | Software memory | Software CPU | GPU memory | GPU CPU | Threads |
| --- | --- | --- | --- | --- | --- |
| No cameras | 14.0 MiB | 0.3% | 88.0 MiB | 0.3% | 15 |
| One remote camera | 19.4 MiB | 5.0% | 92.0 MiB | 5.0% | 15 |
| Two remote cameras | 25.1 MiB | 9.4% | 96.6 MiB | 7.3% | 15 |
| Three remote cameras and Alice's own | 41.4 MiB | 19.0% | 113.1 MiB | 15.4% | 21 |
| Every camera off again | 15.3 MiB | 0.4% | 85.4 MiB | 0.5% | 15 |
| On and off a second time | 15.3 MiB | 0.4% | 85.5 MiB | 0.5% | 15 |

Threads are the Software run's. A remote camera costs about 5.5 MiB, nearly
all of it its decoder's 720p frames, and no thread: one thread decodes every
camera. Alice's own camera costs about 11 MiB and six threads, OpenH264's
encoder threads and the test pattern's. Everything goes when the cameras do:
1.3 MiB stays after the first round and nothing more after the second.
Another worktree's UI session shared the host, so CPU is a little noisy.

Reproduce: `python3 testing/measure_discord_cameras.py --camera CAMERA.h264
--output .local/measure-discord-cameras`.

## 2026-10-13: Discord server and channel menus

The record compares `fa68b5f` with
the server and channel menus stacked on it, measured at `6cf4aa2` before
they were rebased onto the channel header: a server's menu and header
dropdown, category menus, invites, making, changing and deleting channels
and categories, and leaving a server. Menu rows are built only when a menu
opens, the dialogs hold state only while open, and a server's categories
and roles are compared by `Arc` to tell when the sidebar needs its empty
categories again. Discord-only `fast` builds ran on the busy fake account
with the server's #general and its member list open.

| Build | RSS, mean of 3 (range) | PSS, mean of 3 |
| --- | --- | --- |
| `fa68b5f` | 107.1 MiB (106.0–109.2) | 87.7 MiB |
| This change | 106.8 MiB (106.4–107.1) | 87.3 MiB |

The difference is inside the spread between runs of one build, so the
menus cost nothing measurable while idle.

Reproduce: as for the server foundations above, with each build's binary.

## 2026-10-14: A Discord channel's list of threads

The record compares `97b92df` with the
list of threads at `cb6b8ae`. Each active thread the service knows now
keeps its newest message (its id, author and up to 200 bytes of text), its
member count and three member ids: 48 bytes more in the thread, plus about
300 bytes of text and ids, and only for active threads in servers Yapfast
subscribed to. The list keeps its archived pages, search results and the
channel objects it found only while it's open. Both Discord-only `fast`
builds ran on this change's fake with #general open, and each figure is
glibc's heap in use, from the window-control `stats` command, as RSS swung
with other builds on the host.

| Build and state | Heap in use, mean of 3 |
| --- | --- |
| `97b92df`, #general | 7.05 MiB |
| This change, #general | 7.17 MiB (one run 7.40, as `main` gave in an earlier run) |
| This change, list open with all 30 archived threads loaded | +2.52 MiB |
| This change, after the list closes | +1.91 MiB |

Idle with the list never opened, the difference is within the spread
between runs. Most of what the list costs stays after it first closes, but
it doesn't grow with later opens: three opens and closes in a row left
9.31, 9.35 and 9.35 MiB. The pins popover, already in `main`, also leaves
tens of KiB after each open. What stays is most likely Skia's glyph cache:
the list's rows fill it with sizes and weights shown nowhere else, and it
is bounded at 2 MiB by default. This isn't confirmed.

Reproduce: start each build's binary with `scripts/ui-session start
--discord --bin BUILD`, open #general, append `stats` to
`.local/ui-session/window-control`, and read the in-use bytes from
`.local/ui-session/yapfast.log`; for this change, open the list, scroll it to
its end and read again, then close it and read again.

## 2026-10-14: Closing the window to the tray

The record measures the Discord-only
`fast` build on the fake Discord with five servers, 20 more DMs and pictures
in #general (`--servers 4 --dms 20 --images 4`), through
`testing/measure_tray.py`: #general open and idle, then the window closed to
the [tray](../tray.md), which drops it and trims the heap, sampled for a
minute, then opened again from the tray; then the same in the Lounge call.
Memory is anonymous memory plus swap, which is what Yapfast itself holds and
close to the private bytes Windows reports. Heap in use is glibc's. Skia
Software, the default renderer, is the mean of three runs (the range in
brackets); Skia GPU is one run.

| State | Software memory | Software heap in use | GPU memory | GPU heap in use |
| --- | --- | --- | --- | --- |
| #general, window open | 18.6 MiB (18.4–19.0) | 10.2 MiB | 86.6 MiB | 73.1 MiB |
| #general, closed to the tray | 16.3 MiB (16.1–16.5) | 9.1 MiB | 62.9 MiB | 48.8 MiB |
| #general, opened again | 17.1 MiB (17.0–17.4) | 10.1 MiB | 65.5 MiB | 53.8 MiB |
| Lounge call, window open | 17.7 MiB (17.4–18.0) | 10.4 MiB | 71.1 MiB | 54.2 MiB |
| Lounge call, closed to the tray | 16.8 MiB (16.6–17.0) | 9.7 MiB | 68.4 MiB | 49.2 MiB |
| Lounge call, opened again | 16.9 MiB (16.8–17.1) | 10.4 MiB | 70.8 MiB | 53.9 MiB |

With Skia Software closing saves 2.3 MiB (12%) the first time and 0.9 MiB in
the call, and a window opened again holds under 1 MiB more than none. The
rest is the session and what the next window reuses, such as the models,
which this didn't break down further. Releasing the
timeline's rows, the window's picture copies and Skia's caches too saved
0.25 MiB of heap and no anonymous memory, so they stay. With the GPU, the
first close saves 24 MiB, a quarter: the first window's GPU state held heap
that a window made again doesn't, so after that a window costs about
2.5 MiB here as well. CPU while closed was 0–0.02% of one core idle and
0.25% in the call, against 0.2–0.4% open. PSS fell by 6 MiB (Software) and
24 MiB (GPU) on the first close.

Real Windows is unmeasured: there Yapfast compacts the heap and empties the
working set, which Task Manager's Memory column, the private working set,
shows at once.

Reproduce: `python3 testing/measure_tray.py --output .local/measure-tray`,
and with `--runs 1 --renderer gpu` for the GPU column.

## 2026-10-22: Game detection

`yapfast-game-detection-probe` loads a game list, scans this user's
processes once and then 30 times more, and reports the heap in use, from
glibc's `mallinfo2`, before the list loads and after the scans. It runs
without Discord, a window or a call, and prints no process names. Both
builds are the `measure` profile on an AMD Ryzen 9 9900X with 540
processes running, five runs each, alternating. The list is Discord's
`detectables/games-v1.json` of 2026-10-22, compacted as Yapfast caches it:
19,465 games, 11,159 Linux and Windows executable rules.

| | Before | After |
| --- | ---: | ---: |
| Heap held by detection | 5,215 KiB | 154–159 KiB |
| First scan | 5.7–6.9 ms | 26–27 ms |
| Later scans, mean | 2.9–3.0 ms | 4.5–4.6 ms |

Before is the detector as the pull request brought it, which held every
rule and game name of the list, and `sysinfo`'s table of every process
between scans. After holds a sorted hash of each executable file name,
about 80 KB, reads the cached list again for the rules of names it sees,
and drops the process table after each scan. The first scan reads the list
once for every name found; later scans, one every 15 s, list the processes
anew. The first run of each build was slower and is left out of the ranges.

Reproduce: download the list, compact it as `game_detection::catalog`
does (this platform's executables and Steam ids), then

```sh
nix develop path:./nix --command cargo build --locked --profile measure -p yapfast-discord --bin yapfast-game-detection-probe
target/measure/yapfast-game-detection-probe /path/to/discord-games.json 30
```

For the before column, build the parent of the commit that made the
change, with the probe's `rss_kib` replaced by the same `mallinfo2` read.

## 2026-10-23: mimalloc against glibc's malloc

This record asks whether mimalloc, as Rust's global allocator, holds less
memory than glibc. The `mimalloc` Cargo feature makes it the allocator and
calls `mi_collect(true)` wherever Yapfast already calls `malloc_trim`: after
READY, 3 s after a call, and 0.5 s after the window closes to the tray. The
arena cap and the trims stay, since Skia, WebRTC and PulseAudio are C and C++
and keep allocating from glibc. The crate, 0.1.52, builds mimalloc v3.3.2.

Three builds, all Discord-only `measure` builds: glibc (the default),
mimalloc on its defaults, and mimalloc tuned to give memory back promptly,
built with the crate's `no_thp` feature and run with
`MIMALLOC_PURGE_DELAY=0`. They ran through `testing/measure_tray.py` as in
[closing the window to the tray](#2026-10-14-closing-the-window-to-the-tray),
with a sample after leaving the call added, in three rounds that alternate
the builds' order. Memory is anonymous memory plus swap, which covers both
allocators, at the end of each sample: the closed rows are a minute after
the close. Brackets give the range of the three runs. CPU is a percentage of
one core.

| Skia Software | glibc | mimalloc, defaults | mimalloc, tuned |
| --- | ---: | ---: | ---: |
| #general, window open | 19.7 MiB (19.4–19.9) | 55.1 MiB (24.0–72.7) | 23.4 MiB (23.3–23.4) |
| #general, closed to the tray | 16.8 MiB (16.8–16.8) | 56.4 MiB (21.0–80.4) | 20.7 MiB (20.6–20.7) |
| #general, opened again | 17.8 MiB (17.7–17.8) | 58.3 MiB (22.0–81.1) | 21.6 MiB (21.6–21.6) |
| Lounge call, window open | 38.3 MiB (36.2–39.5) | 89.0 MiB (44.6–114.8) | 39.7 MiB (39.0–40.2) |
| Lounge call, closed to the tray | 32.0 MiB (32.0–32.1) | 91.3 MiB (44.0–123.0) | 39.2 MiB (38.4–39.7) |
| Lounge call, opened again | 32.2 MiB (32.1–32.3) | 91.4 MiB (44.1–123.1) | 39.4 MiB (38.6–39.8) |
| After leaving the call | 18.7 MiB (17.4–21.0) | 59.0 MiB (23.3–81.4) | 23.0 MiB (22.9–23.2) |
| CPU in the call, window open | 6.51% (6.24–6.64) | 6.51% (6.34–6.59) | 6.36% (6.04–6.59) |

One Skia GPU run of each, for the case of the post on freed memory:

| Skia GPU, one run | glibc | mimalloc, defaults | mimalloc, tuned |
| --- | ---: | ---: | ---: |
| #general, window open | 89.0 MiB | 139.4 MiB | 92.7 MiB |
| #general, closed to the tray | 65.1 MiB | 115.6 MiB | 69.0 MiB |
| Lounge call, window open | 109.8 MiB | 169.9 MiB | 110.4 MiB |
| Lounge call, closed to the tray | 78.8 MiB | 147.2 MiB | 87.7 MiB |
| After leaving the call | 67.9 MiB | 122.8 MiB | 76.2 MiB |

mimalloc's defaults cost the most, and by how much depends on the machine.
v3 commits its 1 GiB arena up front on Linux and asks for transparent huge
pages on it (`madvise`), so wherever the kernel has free 2 MiB pages, a few
live allocations keep whole 2 MiB pages resident. This host's THP setting is
`madvise`, and its memory was fragmented: two of the three Software runs got
huge pages (the arena held 43–112 MiB), one didn't (12–33 MiB), and all five
startup launches did, with a peak RSS of 165–171 MiB against 102 MiB. Setting
`MIMALLOC_ALLOW_THP=0` turned THP off when it was mimalloc's only variable,
but not when a second `MIMALLOC_` variable was also set (8 of 9 launches), so
the tuned build leaves huge pages out at compile time.

Tuned, mimalloc gave memory back as promptly as glibc's trims do, and closing
the window still returned what it held: 2.7 MiB with Software and 23.7 MiB
with the GPU, against 2.9 and 23.9 on glibc. Neither build gave back more
during the minute closed than in its first 5 s. Yet the tuned build held
more than glibc in every state, by 3.7 MiB with #general open, 4.3 MiB after
the call, and 7.2 MiB in the call with the window closed, where glibc's trim at the
close returned 6.3 MiB of heap the call had freed. The runs of each build differ by about
0.5 MiB idle and 3 MiB in the call. PSS shows the same, 102.5 against
98.7 MiB idle.

How much is Rust's: on the mimalloc builds glibc's heap holds only the C and
C++ allocations, 2.4 MiB in use idle and 3.4 MiB in the call against 10.1
and 21.2 MiB on glibc, so Rust has about 8 MiB in use idle and 18 MiB in the
call. With the GPU, glibc keeps about 65 MiB of driver and Skia heap on any
build. The tuned arena held 11.6 MiB idle and 28.5 MiB in the call for those.

Startup, five launches of each, alternated, was the same for all three: the
CPU went quiet 0.13–0.19 s after launch, after 0.11–0.17 s of CPU, and
used 0.19–0.23 s of CPU in the first 3 s. The stripped binary grew by
146,584 bytes (0.1%). This Discord-only build runs 15 threads idle and 21
in the call.

With mimalloc, the window control's `stats` still prints only glibc's heap;
the script reads mimalloc's from the anonymous memory of its arena, the
mappings of 256 MiB or more. mimalloc's own committed counter isn't usable
for this: with the arena committed up front it stayed at 142 MiB.

Windows is unmeasured. There Rust already allocates from the system heap,
not glibc, so the comparison is a different one, and Wine's heap isn't
Windows', so it needs a real Windows machine.

Reproduce: build each variant and copy it out, then run each in turn,
alternating the order between rounds:

```sh
nix develop path:./nix --command cargo build --locked --profile measure --no-default-features --features desktop,discord --bin yapfast
# mimalloc: add ,mimalloc to the features; tuned: add ,mimalloc,mimalloc/no_thp
python3 testing/measure_tray.py --runs 1 --bin /tmp/alloc/glibc/yapfast --output .local/alloc/glibc/round1
python3 testing/measure_tray.py --runs 1 --bin /tmp/alloc/tuned/yapfast --env MIMALLOC_PURGE_DELAY=0 --output .local/alloc/tuned/round1
```

Add `--renderer gpu` for the GPU table.

## 2026-10-23: glibc and mimalloc tuned, without the arena cap

This record asks the questions replies to the post on freed memory raised
about [the record above](#2026-10-23-mimalloc-against-glibcs-malloc): does
the cap of two arenas cost lock contention, does tuning the allocator
through its own settings make Yapfast's `malloc_trim` calls unneeded, and how
does mimalloc compare once neither allocator is capped. Two environment
variables, experiments, turn the app's own settings off without a rebuild:
`YAPFAST_NO_ARENA_CAP` skips `mallopt(M_ARENA_MAX, 2)`, as an `arena_max` in
`GLIBC_TUNABLES` now also does, and `YAPFAST_NO_TRIM` skips the trims after
READY, after a call and after closing to the tray. Neither they nor the `mimalloc`
feature stayed in the tree; `.local/alloc2/experiment.patch` holds them.

The settings, chosen from glibc 2.40's source and manual and mimalloc
v3.3.2's options, then a pilot of one short run each:

- glibc tuned: `GLIBC_TUNABLES=glibc.malloc.mmap_threshold=131072`. Without
  it glibc raises the threshold for its own mappings to the size of each
  mapped block that's freed, up to 32 MiB, and the trim threshold to twice
  that; after the first decoded picture, every later picture buffer lands in
  an arena and stays there. Setting any one of `mmap_threshold`,
  `trim_threshold`, `top_pad` or `mmap_max` stops that adjustment for good;
  128 KiB is glibc's own starting value, and the trim threshold stays at its
  128 KiB. In the pilot `top_pad=0` and `mxfast=0` changed nothing beyond the
  noise, and a 32 KiB threshold under 1 MiB, so they stay at glibc's
  defaults. `arena_test` caps nothing: it only says how many arenas glibc
  makes before it works out its limit of eight per core, here 192.
- mimalloc tuned: the `mimalloc,mimalloc/no_thp` build with
  `MIMALLOC_PURGE_DELAY=0` and glibc's setting above for the C and C++ that
  stays on glibc. v3 purges a freed page only when a later free or
  allocation runs past its delay, which defaults to 1000 ms; it has no
  timer, so what an idle app frees waits. In the pilot, without trims, a
  1000 ms delay held 36.8 MiB in mimalloc's arena in the call, 10 ms 29.4, and
  0 ms 27.8; 0 ms cost twice the `madvise` calls of 10 ms and no CPU that
  showed. `MIMALLOC_PAGE_FULL_RETAIN=0` and `MIMALLOC_GENERIC_COLLECT=1000`
  saved nothing.

Memory ran through `testing/measure_tray.py` as in the record above, three
rounds of Skia Software in an order that rotates, and one Skia GPU run each.
"No cap, glibc defaults" keeps the trims; the tuned builds ran with and
without them. Brackets give the range of the three runs, which is the noise
floor: within 0.5 MiB for most rows.

| Skia Software, MiB | Today: cap 2, trims | No cap, glibc defaults | glibc tuned, trims | glibc tuned, no trims | mimalloc tuned, trims | mimalloc tuned, no trims |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| #general, window open | 19.4 (19.3–19.5) | 22.6 (22.5–22.8) | 18.0 (17.9–18.1) | 18.7 (18.6–18.8) | 21.2 (21.1–21.3) | 21.1 (21.0–21.2) |
| #general, closed to the tray | 16.7 (16.7–16.8) | 17.6 (17.4–17.7) | 17.4 (17.4–17.4) | 18.7 (18.6–18.8) | 20.9 (20.8–21.0) | 21.0 (21.0–21.1) |
| #general, opened again | 17.7 (17.7–17.8) | 18.6 (18.4–18.7) | 17.7 (17.7–17.8) | 18.8 (18.7–18.8) | 21.2 (21.1–21.3) | 21.2 (21.1–21.3) |
| Lounge call, window open | 39.2 (39.1–39.3) | 41.1 (40.9–41.3) | 29.9 (29.8–29.9) | 30.9 (30.7–31.2) | 38.9 (38.8–38.9) | 38.8 (38.6–38.9) |
| Lounge call, closed to the tray | 31.8 (31.8–31.9) | 38.0 (37.9–38.1) | 29.6 (29.5–29.7) | 30.9 (30.7–31.2) | 38.7 (38.5–38.8) | 38.8 (38.6–38.9) |
| Lounge call, opened again | 32.0 (31.9–32.1) | 38.1 (38.0–38.3) | 29.8 (29.7–29.8) | 30.9 (30.7–31.2) | 38.8 (38.7–38.9) | 38.9 (38.8–39.0) |
| After leaving the call | 23.2 (22.8–23.5) | 28.5 (28.2–29.0) | 18.1 (18.1–18.1) | 20.7 (20.4–21.1) | 23.1 (22.9–23.2) | 23.4 (23.3–23.5) |
| PSS, #general open | 98.4 (98.3–98.5) | 98.7 (92.6–102.0) | 94.7 (89.6–97.6) | 97.7 (97.4–98.1) | 100.5 (100.2–100.7) | 97.4 (92.1–100.4) |
| PSS, in the call | 138.0 (137.8–138.1) | 137.1 (131.0–140.1) | 128.9 (128.6–129.2) | 129.7 (129.5–130.0) | 135.0 (129.1–137.9) | 137.7 (137.2–138.0) |
| CPU in the call, window open | 6.36% (6.34–6.39) | 6.37% (6.24–6.49) | 6.29% (6.29–6.29) | 6.31% (6.09–6.44) | 6.07% (5.84–6.24) | 6.36% (6.34–6.39) |

| Skia GPU, one run, MiB | Today | No cap, defaults | glibc tuned, trims | glibc tuned, no trims | mimalloc tuned, trims | mimalloc tuned, no trims |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| #general, window open | 88.8 | 91.1 | 86.5 | 86.8 | 90.3 | 90.1 |
| #general, closed to the tray | 65.2 | 64.7 | 64.4 | 83.2 | 68.7 | 86.6 |
| Lounge call, window open | 100.1 | 114.3 | 97.9 | 100.3 | 108.8 | 110.1 |
| Lounge call, closed to the tray | 76.9 | 81.8 | 76.3 | 95.4 | 87.4 | 105.1 |
| After leaving the call | 73.2 | 81.8 | 67.2 | 85.9 | 75.6 | 92.0 |

Without the cap glibc made 22–23 arenas here, one for each thread that
allocates. On its default thresholds that cost 0.9–6.2 MiB against the cap,
so the cap did save memory, but the fixed mapping threshold saves more
without it: 9.3 MiB less than today in the call, 5.1 MiB less after it.
Heap in use was the same in all four glibc columns; glibc's held heap in the
call fell from 32.5 MiB (capped) to 25.1.

The trims still matter. A tuned threshold gives back the mapped blocks and
the free space at the top of each arena, but nothing freed below a block
still in use; only `malloc_trim` does that. With Skia Software, going
without them cost 0.7 to 2.6 MiB on tuned glibc. With the GPU, closing the window gave
back 22.1 MiB with the trims and 3.6 MiB without, and in the call 21.6
against 4.9: what the window frees there is mostly the driver's and Skia's,
C and C++ on glibc in either build, so a mimalloc build needs glibc's trim as
much (21.6 against 3.5). mimalloc's own trims changed nothing with a 0 ms
purge delay. Closing the window, on Software, gave back 2.7 MiB today and
0.6 MiB tuned: the tuned build had already given back the mapped picture
buffers while the window was open.

Tuned mimalloc held 2–9 MiB more than tuned glibc in every state. Its arena
held 11.7 MiB idle and 27.9 MiB in the call for Rust's 7.6 and 17.8 MiB in
use (glibc's in-use heap, 10.1 and 21.3, less the C and C++ part left on
glibc in the mimalloc build, 2.5 and 3.5).

Performance came from a run against the fake that `.local/alloc2/perf_run.py`
drives, uncommitted, with `testing/fake_discord.ts`'s new
`/_control/burst` and `/_control/speakers-start`: launch until the CPU is
quiet; 1,500 messages from 40 people within 0.1 s, a third in the open
#general, every fourth of those with a 960x640 picture and every fifth with
an embed's picture, the rest across eight more channels and three servers;
switching through those eight channels, each opened at 60 messages with
pictures, scrolling 2,400 pixels down in each; then 30 s in the Lounge call
with Bob, Carol and Dave speaking at once. Each variant ran three rounds
on all 24 CPUs, and pinned with `taskset` to four cores (CPUs 8–11) and to
two (8–9), in rotating order; the trims stayed on. Whole-run figures are
the process's own rusage at exit, every thread's. Brackets give the range,
the noise floor.

| All 24 CPUs | Today: cap 2 | No cap, defaults | glibc tuned | mimalloc tuned |
| --- | ---: | ---: | ---: | ---: |
| Burst, CPU, s | 0.28 (0.23–0.31) | 0.32 (0.30–0.34) | 0.32 (0.30–0.34) | 0.34 (0.32–0.36) |
| Burst, until quiet, s | 0.65 (0.62–0.67) | 0.62 (0.62–0.62) | 0.62 (0.62–0.62) | 0.62 (0.62–0.62) |
| Channels, CPU, s | 3.38 (3.33–3.43) | 3.45 (3.39–3.55) | 3.40 (3.38–3.41) | 3.27 (3.22–3.31) |
| Call with three speaking, CPU per 30 s | 1.97 (1.95–2.02) | 1.93 (1.92–1.96) | 2.00 (1.97–2.02) | 1.94 (1.92–1.96) |
| Whole run, user, s | 6.75 (6.64–6.84) | 6.70 (6.57–6.84) | 6.69 (6.65–6.72) | 6.50 (6.42–6.57) |
| Whole run, sys, s | 0.15 (0.11–0.19) | 0.20 (0.19–0.21) | 0.26 (0.18–0.32) | 0.24 (0.21–0.27) |
| Whole run, minor faults, thousands | 36.2 (32.0–42.1) | 44.5 (43.6–45.2) | 142 (102–219) | 98.4 (95.2–100.3) |
| Whole run, voluntary context switches, thousands | 35.7 (35.0–36.8) | 33.1 (33.1–33.1) | 33.7 (33.2–34.1) | 32.7 (32.5–32.9) |
| Peak RSS, MiB | 166 (164–169) | 210 (207–213) | 162 (161–162) | 194 (193–194) |
| glibc arenas at the end | 2 | 35 (32–38) | 35 (34–37) | 33 (32–34) |

| Four cores, then two | Today: cap 2 | No cap, defaults | glibc tuned | mimalloc tuned |
| --- | ---: | ---: | ---: | ---: |
| 4: whole run, user, s | 7.05 (6.74–7.32) | 7.24 (7.20–7.31) | 7.23 (7.17–7.26) | 7.04 (6.98–7.15) |
| 4: whole run, sys, s | 0.47 (0.19–0.76) | 0.17 (0.15–0.20) | 0.20 (0.16–0.23) | 0.22 (0.19–0.24) |
| 4: call, CPU per 30 s | 2.22 (2.19–2.24) | 2.12 (2.05–2.17) | 2.16 (2.15–2.19) | 2.08 (2.06–2.10) |
| 4: voluntary switches, thousands | 35.1 (33.2–36.3) | 32.3 (32.0–32.8) | 32.3 (32.0–32.5) | 31.9 (31.7–32.1) |
| 4: peak RSS, MiB | 167 (163–169) | 227 (224–229) | 164 (164–164) | 202 (197–206) |
| 2: whole run, user, s | 7.28 (7.19–7.35) | 7.01 (6.83–7.29) | 7.29 (7.20–7.46) | 7.04 (7.00–7.08) |
| 2: whole run, sys, s | 0.27 (0.24–0.28) | 0.22 (0.14–0.27) | 0.31 (0.25–0.34) | 0.26 (0.25–0.27) |
| 2: call, CPU per 30 s | 2.25 (2.23–2.29) | 2.12 (2.00–2.20) | 2.29 (2.19–2.37) | 2.16 (2.14–2.19) |
| 2: voluntary switches, thousands | 34.6 (33.3–36.7) | 33.4 (32.3–35.0) | 32.9 (32.7–33.1) | 32.8 (32.2–33.2) |
| 2: peak RSS, MiB | 168 (166–171) | 231 (228–236) | 165 (164–166) | 218 (208–224) |

One run of each under `strace -f`, on all CPUs, for the system calls:

| Calls in the whole run | Today: cap 2 | No cap, defaults | glibc tuned | mimalloc tuned |
| --- | ---: | ---: | ---: | ---: |
| futex waits on a held lock | 3,699 | 983 | 1,215 | 779 |
| futex, all | 69,418 | 62,884 | 62,976 | 62,283 |
| mmap / munmap | 545 / 76 | 579 / 95 | 1,426 / 886 | 627 / 143 |
| brk / mremap | 90 / 1 | 62 / 1 | 129 / 176 | 109 / 1 |
| madvise (MADV_DONTNEED) | 57 | 67 | 283 | 1,164 |
| mprotect | 743 | 1,108 | 1,250 | 370 |

The cap does cost contention: three times the waits on a held lock (a futex
wait on value 2, as glibc's lock and Rust's `Mutex` both wait), and 1,200
to 3,000 more voluntary context switches over the run. It didn't show as CPU,
nor as time on any number of cores: the burst settled in 0.62–0.67 s
everywhere and the CPU of each phase stayed within the noise. A profile
(`perf record -e cycles:u`, one run each) put glibc's malloc and free at
7.6–7.8% of user CPU with the cap or without; the waiting itself happens in
the kernel. mimalloc and the glibc left beside it took 4.3%: on all CPUs
its runs used 3% less user CPU than the others, and pinned they were within
the others' noise.

The tuned threshold costs what it was set for: picture buffers are mapped
and unmapped each time: three to five times the minor faults of today's
build, and 0.1 s more system time over a run on all CPUs, within the noise
pinned; against 3–4 MiB less peak RSS than today and 48–66 MiB less than
glibc's defaults without the cap. Its minor faults varied most, 102 to 219
thousand.

Startup was 0.2 s to quiet with 0.1 s of CPU in every memory run, the same
for all.

Reproduce: build each and copy it out, then run the scripts in
`.local/alloc2/`, which alternate the variants in a scratch worktree's
ui-session:

```sh
nix develop path:./nix --command cargo build --locked --profile measure --no-default-features --features desktop,discord --bin yapfast
# mimalloc: --features desktop,discord,mimalloc,mimalloc/no_thp
.local/alloc2/mem_matrix.sh software 3 ref g-nocap gt-trims gt mt-trims mt
.local/alloc2/mem_matrix.sh gpu 1 ref g-nocap gt-trims gt mt-trims mt
.local/alloc2/perf_matrix.sh 3
.local/alloc2/perf_trace.sh
```

`.local/alloc2/variants.sh` names each variant's binary and environment.

### The cap with the fixed threshold

Cap and threshold together, `mallopt(M_ARENA_MAX, 2)` with the same
`GLIBC_TUNABLES`, held the least of all, but its main arena grew and shrank
with `brk` 8,638 times in the run under `strace` (90 today). heaptrack
showed why: DeepFilterNet, on tract, allocates two 120 KiB tensors in each
10 ms block of the microphone and frees them. Under the cap the capture
thread shares the main arena, whose top is trimmed whenever 128 KiB lies
free there, the fixed trim threshold; an arena of its own trims only past
twice that, and on glibc's defaults the trim threshold had risen with the
mapping threshold. `glibc.malloc.trim_threshold=524288` stops it: 101 `brk`
calls. The same heaptrack run counted 250,000 allocations a second in a
call, 85% of all of them from DeepFilterNet's tract plan, about 2,500 and
1.25 MiB for each 10 ms block.

Three alternating Software runs each, beside new runs of the uncapped
threshold, which matched its runs above within 0.1 MiB:

| Skia Software, MiB | Today: cap 2 | Threshold, no cap | Cap + threshold | Cap + threshold + 512 KiB trim threshold |
| --- | ---: | ---: | ---: | ---: |
| #general, window open | 19.4 (19.3–19.5) | 18.1 (18.0–18.1) | 17.3 (17.1–17.4) | 17.3 (17.2–17.3) |
| #general, closed to the tray | 16.7 (16.7–16.8) | 17.4 (17.4–17.5) | 16.8 (16.6–16.9) | 16.8 (16.6–16.9) |
| #general, opened again | 17.7 (17.7–17.8) | 17.7 (17.7–17.8) | 17.1 (16.9–17.2) | 17.0 (16.9–17.1) |
| Lounge call, window open | 39.2 (39.1–39.3) | 30.0 (29.8–30.2) | 29.2 (29.1–29.3) | 29.7 (29.4–30.2) |
| Lounge call, closed to the tray | 31.8 (31.8–31.9) | 29.5 (29.5–29.6) | 28.6 (28.6–28.7) | 28.9 (28.8–29.1) |
| Lounge call, opened again | 32.0 (31.9–32.1) | 29.7 (29.7–29.8) | 28.8 (28.7–28.8) | 29.1 (29.0–29.2) |
| After leaving the call | 23.2 (22.8–23.5) | 18.1 (18.1–18.1) | 17.4 (17.2–17.5) | 17.9 (17.4–18.2) |

The 512 KiB trim threshold costs 0.3–0.5 MiB in and after the call, what
the main arena may now keep free at its top until a trim. With Skia GPU,
one run each, the three tuned columns were within 1.7 MiB of each other in
every state but #general opened again, where the app's own heap in use
differed by 19 MiB between runs.

Performance, in rotating order with today's build and the uncapped
threshold, each run waiting for a load average under 1: other worktrees'
sessions kept the machine busier than for the runs above, so every column
used more CPU than there. Whole run unless named:

| | Today: cap 2 | Threshold, no cap | Cap + threshold + 512 KiB |
| --- | ---: | ---: | ---: |
| All CPUs: user, s | 8.27 (8.10–8.47) | 8.40 (7.89–8.95) | 8.11 (7.77–8.68) |
| All CPUs: sys, s | 0.30 (0.24–0.40) | 0.35 (0.29–0.42) | 0.33 (0.29–0.38) |
| All CPUs: call, CPU per 30 s | 2.52 (2.40–2.59) | 2.77 (2.42–2.96) | 2.46 (2.36–2.57) |
| All CPUs: burst, until quiet, s | 0.64 (0.62–0.67) | 0.67 (0.67–0.68) | 0.66 (0.62–0.73) |
| All CPUs: minor faults, thousands | 35.1 (30.8–40.7) | 101 (99–104) | 94.0 (91.7–97.4) |
| All CPUs: peak RSS, MiB | 164 (162–166) | 161 (161–162) | 158 (158–159) |
| 4 cores: user, s | 7.74 (7.25–8.12) | 7.36 (7.32–7.42) | 7.31 (7.17–7.51) |
| 4 cores: peak RSS, MiB | 170 (166–173) | 163 (162–163) | 160 (159–161) |
| 2 cores: user, s | 7.00 (6.90–7.10) | 8.03 (7.03–10.01) | 7.04 (6.56–7.36) |
| 2 cores: peak RSS, MiB | 166 (163–168) | 165 (165–166) | 160 (159–160) |

Under `strace`, one run: 3,829 waits on a held lock (today 3,699), 101
`brk`, 1,261 `mmap` and 793 `munmap`, 70 `madvise`. The CPU stayed within
the noise of today's build on every number of cores, with the lowest peak
RSS of the three; the minor faults are the fixed threshold's, as above.

The capped build with both thresholds holds 0.2–0.8 MiB less than the
uncapped threshold in every state, but Yapfast ships the uncapped one: a
fixed 128 KiB mapping threshold, set with `mallopt` in `main`, glibc's own
count of arenas, and the trims. It gives up those 0.2–0.8 MiB for a third of
the cap's lock waits and no trim threshold sized around one library's
buffers. Against the cap it replaces, it holds 9.3 MiB less in the call and
5.1 MiB less after it.

The shipped build, with `mallopt(M_MMAP_THRESHOLD, 128 * 1024)` instead of
the environment, ran five times beside three new runs of the uncapped
threshold. Three of its runs matched those within 0.2 MiB in every state:
17.9 MiB with #general open, 29.7 in the call, 18.0 after it. In the first
two the app's heap in use was 0.7–1.0 MiB higher, with the same screens and
the same memory over heap in use, 8.0–9.3 MiB in each state: something in the
app, not the allocator.

```sh
.local/alloc2/run3.sh   # cap + threshold against the uncapped threshold
.local/alloc2/run4.sh   # with the 512 KiB trim threshold
```

## 2026-10-25: x264 in place of OpenH264

Go Live's and the camera's H264 encoder, alone: a harness fed the first
seconds of 1080p clips through `Encoder`, OpenH264 as it shipped (screen
content mode, frame skipping on) against x264 in `native/h264.c` at its
default effort (superfast) and with "Higher quality" (faster). The clips are
Xiph's AOM test clips Wikipedia (scrolling, 30 fps), Debugging (an IDE, 30
fps), MissionControlClip3 (a desktop, 60 fps), MINECRAFT and DOTA2 (60 fps),
and a zooming fractal (60 fps). A frame OpenH264 skipped counts as the one
before it shown again, as a viewer sees it. VMAF and PSNR against the clip;
cores are CPU time over the clip's length, with four encoder threads, on a
12-core Ryzen 9 9900X.

At 9 Mbit/s, the cap above 720p:

| Clip | OpenH264: fps, VMAF, cores | x264: fps, VMAF, cores | Higher quality: fps, VMAF, cores |
| --- | ---: | ---: | ---: |
| Minecraft | 20, 38, 0.63 | 60, 66, 0.48 | 60, 72, 1.19 |
| Dota 2 | 54, 74, 0.63 | 60, 78, 0.40 | 60, 82, 0.95 |
| Fractal | 34, 55, 0.40 | 60, 68, 0.38 | 60, 70, 0.87 |
| Desktop | 57, 94, 0.59 | 60, 94, 0.31 | 60, 94, 0.71 |
| Wikipedia | 30, 98 (42.6 dB), 0.28 | 30, 99 (47.7 dB), 0.19 | 30, 99 (50.6 dB), 0.42 |
| IDE | 30, 93 (42.4 dB), 0.27 | 30, 97 (54.2 dB), 0.16 | 30, 97 (53.6 dB), 0.30 |

At 3.5 Mbit/s, the cap at 720p and below:

| Clip | OpenH264 | x264 | Higher quality |
| --- | ---: | ---: | ---: |
| Minecraft | 6, 19, 0.21 | 60, 43, 0.40 | 60, 45, 0.90 |
| Dota 2 | 18, 39, 0.29 | 60, 53, 0.32 | 60, 57, 0.70 |
| Fractal | 13, 25, 0.19 | 60, 18, 0.34 | 60, 52, 0.76 |
| Desktop | 50, 88, 0.52 | 60, 74, 0.27 | 60, 78, 0.56 |
| Wikipedia | 29, 98 (42.1 dB), 0.28 | 30, 94 (36.5 dB), 0.15 | 30, 98 (41.6 dB), 0.30 |
| IDE | 29, 92 (38.5 dB), 0.26 | 30, 95 (43.9 dB), 0.15 | 30, 95 (43.9 dB), 0.31 |

OpenH264 also ran over its bitrate on games, to 11 Mbit/s for Minecraft's 9;
x264 stayed within it on every clip. On calm screens at 3.5 Mbit/s the
default effort scores below OpenH264, which spent fewer bits there and
dropped a few frames; "Higher quality" matches it. On calm screens at 9
Mbit/s x264 uses up to 8 Mbit/s where OpenH264 stayed under 2.

Memory, the harness's peak RssAnon encoding 1080p60, the same on all three
clips tried (the screenshot, the fractal, Minecraft): OpenH264 62 MiB, x264
41 MiB, with "Higher quality" 52 MiB. The stream worker's own figure under
"Sharing your screen" above wasn't measured again.

```sh
cat .local/x264/README   # clips, harness and scripts
```
