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
