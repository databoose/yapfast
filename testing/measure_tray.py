#!/usr/bin/env python3
"""Measure the desktop client as its window closes to the tray and opens again.

Alice reads #general on testing/fake_discord.ts, with a few more servers and
DMs, while the window is open and idle, after it closed to the tray (which
drops the window and trims the heap), and after the tray opened it again;
then the same in the Lounge call. Only Yapfast's process is sampled.

Memory is anonymous memory plus swap, which is what Yapfast itself holds and
close to the private bytes Windows shows. Heap in use is glibc's, from the
window control's `stats`.

Runs Yapfast through scripts/ui-session --tray, which builds the Discord-only
`fast` build and runs testing/tray_host.rs as the tray, on a private display
and bus.
"""
import argparse
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import time

ROOT = Path(__file__).resolve().parent.parent
SESSION = ROOT / '.local/ui-session'
parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
parser.add_argument('--fake-args', default='--servers 4 --dms 20 --images 4')
parser.add_argument('--renderer', choices=['gpu', 'software'], default='software')
parser.add_argument('--runs', type=int, default=3)
parser.add_argument('--warmup', type=int, default=15, help='seconds before each sample')
parser.add_argument('--seconds', type=int, default=20, help='sample length while open')
parser.add_argument('--closed-seconds', type=int, default=60, help='sample length while closed')
parser.add_argument('--no-call', action='store_true', help='skip the call')
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--no-build', action='store_true', help="run the session's last build")
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)


def session(*arguments):
    subprocess.run([str(ROOT / 'scripts/ui-session'), *arguments], check=True, cwd=ROOT, stdout=subprocess.DEVNULL)


def field(path, key):
    return next(int(line.split()[1]) for line in Path(path).read_text().splitlines()
                if line.startswith(key)) / 1024


def heap_in_use():
    """glibc's heap in use, in MiB, from the last `stats` in Yapfast's log."""
    log = SESSION / 'yapfast.log'
    before = log.stat().st_size
    session('do', 'window stats')
    text = log.read_text(errors='replace')[before:]
    totals = re.findall(r'Total \(incl\. mmap\):\s*system bytes\s*=\s*(\d+)\s*in use bytes\s*=\s*(\d+)', text)
    return round(int(totals[-1][1]) / 2**20, 2) if totals else None


def sample(name, seconds, notes):
    pid = int((SESSION / 'yapfast.pid').read_text())
    result = json.loads(subprocess.check_output(['python3', str(ROOT / 'scripts/measure-process'), str(pid),
                                                 '--seconds', str(seconds)], text=True))
    anonymous = field(f'/proc/{pid}/smaps_rollup', 'Anonymous:') + field(f'/proc/{pid}/status', 'VmSwap:')
    result.update(profile=name, anonymous_mib=round(anonymous, 1), heap_in_use_mib=heap_in_use(),
                  threads=len(os.listdir(f'/proc/{pid}/task')), **notes)
    print(json.dumps({key: result[key] for key in ('profile', 'run', 'anonymous_mib', 'heap_in_use_mib',
                                                    'pss_mib_mean', 'rss_mib_mean', 'cpu_percent_one_core',
                                                    'threads')}), flush=True)
    return result


def cycle(prefix, notes, results):
    """Open and idle, closed to the tray, and opened again."""
    time.sleep(args.warmup)
    shot = prefix.lstrip('#').replace(' ', '-').lower()
    session('do', f'shot {shot}-open')
    results.append(sample(f'{prefix}, window open', args.seconds, notes))
    session('do', 'window close')
    time.sleep(5)
    results.append(sample(f'{prefix}, closed to the tray', args.closed_seconds, notes))
    session('do', 'tray activate')
    time.sleep(args.warmup)
    session('do', f'shot {shot}-reopened')
    results.append(sample(f'{prefix}, opened again', args.seconds, notes))


def keep_shots(run):
    # The next session's start empties the folder.
    for shot in (SESSION / 'shots').glob('*.png'):
        shot.replace(args.output / f'run{run}-{shot.name}')


results = []
build = not args.no_build
try:
    for run in range(1, args.runs + 1):
        subprocess.run([str(ROOT / 'scripts/ui-session'), 'stop'], cwd=ROOT, capture_output=True)
        session('start', '--discord', '--tray', '--fake-args', args.fake_args, '--renderer', args.renderer,
                *([] if build else ['--no-build']))
        build = False
        notes = {'renderer': args.renderer, 'run': run}
        session('do', 'sleep 3', 'click 36 101', 'sleep 2', 'click 142 149')  # Server, #general
        cycle('#general', notes, results)
        if not args.no_call:
            session('do', 'click 142 221')  # Lounge, which joins it
            cycle('Lounge call', notes, results)
            session('do', 'tray click Disconnect')
        keep_shots(run)
finally:
    subprocess.run([str(ROOT / 'scripts/ui-session'), 'stop'], cwd=ROOT, capture_output=True)
    (args.output / 'results.json').write_text(json.dumps({
        'recorded_on': time.strftime('%Y-%m-%d'),
        'binary': str(ROOT / 'target/fast/yapfast'),
        'os': f'{platform.system()} {platform.release()}', 'machine': platform.machine(),
        'isolation': 'scripts/ui-session --tray: private headless Weston, PulseAudio and D-Bus, '
                     'with testing/tray_host.rs as the tray',
        'remote_load': f'testing/fake_discord.ts {args.fake_args}',
        'samples': results,
    }, indent=2) + '\n')
    print(f'Results {args.output / "results.json"}', flush=True)
