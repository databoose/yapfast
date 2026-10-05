# Resource measurements

[Linux starter baseline, 2026-10-01](2026-10-01-linux-starter.json) measures the
empty release build-check window after 15 seconds of warm-up, inactive at a
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

Reproduce process sampling with `scripts/measure-process PID --seconds 30`.
Record a new profile when real account sync and calling are active. Keep hardware,
workload, warm-up, window activity, and memory metric attached to every result.

[Native voice audio, 2026-10-02](2026-10-02-linux-voice-audio.json) is a
functional two-process virtual-device result, rather than a performance sample.
It checks encrypted capture/playback after a 30-second wait alone, both audio
directions, mute/deafen and clean departure. Physical listening and Windows
remain untested.

[Waiting alone in voice, debug build, 2026-10-02](2026-10-02-linux-voice-waiting-debug.json)
samples a real account with one muted participant after confirming encryption
and waiting at least 15 seconds. Mean RSS was 324.1 MiB, mean PSS 307.9 MiB,
and CPU 1.03% of one core over 30 seconds. GPU counters were unavailable.
This debug build uses line-table symbols. It cannot be compared directly with
the empty release starter above and does not establish the beta budgets,
battery behavior or game interference. Release measurements on the reference
Linux and Windows machines remain needed.

[Connected release client, 2026-10-03](2026-10-03-linux-connected.json) samples
the Slint release build with a real account on the local stack, on a private
headless Weston desktop with private PulseAudio null devices. Each profile
warms up for 30 seconds, then samples Talk's process for 60 seconds at 1120×760.
A separate load process runs Bob and eight more accounts with generated tones,
so only Talk's own memory is counted.

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

[Connected client after Milestone 2 and the UI overhaul, 2026-10-04](2026-10-04-linux-connected-overhaul.json)
repeats the 2026-10-03 run on the same host, with the same workload, warm-up and
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
window, Talk anchors it to the bottom, so each new message moves every row.

The ten-person call stays below 400 MiB. Idle PSS meets the 250 MiB target with
both renderers, and so does Software RSS. GPU RSS is now 6.3 MiB over with an
empty channel and 19.6 MiB over with the seeded history; it was 2.8 MiB over
before. RSS counts shared graphics-driver libraries in full. The measurement
limits listed for the 2026-10-03 run still apply.

[Joining a 20-person voice channel, 2026-10-05](2026-10-05-linux-voice-join-20.json)
measures joins with one process per person (`testing/join_voice.py`). Alice runs
the release Talk window, muted. The other 19 are separate release processes on
the same voice stack. Each one publishes a tone in turn, sends digital silence
with DTX the rest of the time, and decodes every stream it receives. People join
12 s apart, so each arrival finds a settled call and rotates the media key. At
full size one person leaves and rejoins three times. Talk, every participant,
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
the Talk window used 3.5% of one core and 243 MiB PSS (307 MiB RSS). Each
headless participant used 3.6% and 26 MiB PSS (98 MiB RSS) without any UI.
Steady state had no decryption failures and no participant who could not hear
another. The 197 missing-key events came from audio that arrived just before its
key during joins. Talk now shows a missing key only once it has lasted beyond the
10 s grace period.

On a real network every round trip adds latency, so absolute join times will be
longer than here. Windows, real devices and a remote homeserver remain unmeasured.

[A call with the window hidden or destroyed, 2026-10-05](2026-10-05-linux-window-background.json)
measures what a tray could save (`testing/measure_background.py`). Talk has no
tray yet, so a test-only window control (`TALK_UI_TEST_CONTROL`) hides the window,
destroys it and creates it again while Talk stays in the call. Alice runs the
release Talk window with GPU rendering, the sample conversation loaded, unmuted
in a ten-person call. The other nine are headless participant processes, as in
the join measurement. Each state settles for 30 s and is sampled for 60 s.

| Talk in a ten-person call | PSS | RSS | CPU, one core |
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
Talk's memory is not tied to the window. Of the 234 MiB PSS with the window open,
93 MiB is resident code from Talk's own binary (Skia, WebRTC, Slint and the Matrix
SDK), 48 MiB is the GPU driver, and 86 MiB is heap and anonymous memory. Destroying
the window unmaps neither the code nor the driver. Talk's anonymous memory falls
from 98 MiB to 73 MiB after the trim. A headless participant with the same voice
stack has 15 MiB. Its RSS counts the shared test binary in full, and its CPU leaves
out audio devices and audio processing.

So a tray that hides the window keeps Talk's full cost. Destroying the window
and trimming the heap saves about a tenth. A small background process for the
session and call, with the window in a separate process started on demand,
would come close to the headless figures.

[Where the memory goes: renderer and allocator, 2026-10-05](2026-10-05-linux-memory-renderer-allocator.json)
repeats the window measurement with glibc's allocator statistics (`malloc_stats`)
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

The renderer decides most of Talk's private memory. With GPU rendering the heap
holds 78 MiB in use, against 18 MiB with Skia Software. The difference comes from
the graphics driver and Skia's GPU state, not from the chat or the call. Hiding
the window frees 24 MiB of that heap, but glibc keeps it, and it only goes back
to the system after `malloc_trim`. In this voice view both renderers used the
same CPU.

glibc gave Talk's threads 47 arenas, which held 13 MiB more than was in use.
Talk now caps them at two at startup unless `MALLOC_ARENA_MAX` is set. That
removed the gap and 8 MiB of anonymous memory at the same CPU. PSS varies by
about 10 MiB between runs of the same build, so the allocator columns are the
better comparison. Lock contention in real-time audio threads was not measured
beyond CPU and a clean call.
