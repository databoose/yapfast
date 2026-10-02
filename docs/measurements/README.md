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
