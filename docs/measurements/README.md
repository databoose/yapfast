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
