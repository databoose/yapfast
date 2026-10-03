#!/usr/bin/env python3
"""Check automatic GPU-to-Skia-Software recovery on the private compositor.

The GPU start is made to fail by hiding every EGL vendor from libglvnd, which is
how a missing or broken OpenGL driver appears to the window. Each scenario uses
its own data folder and captures the window for visual review.
"""
import argparse
import json
import os
import re
from pathlib import Path
import runpy
import signal
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent
INPUT = runpy.run_path(str(ROOT / 'scripts/framework-input'))
INPUT['require_private_display']()
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--exe', type=Path, default=ROOT / 'target/debug/talk')
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
BROKEN_EGL = {'__EGL_VENDOR_LIBRARY_FILENAMES': '/nonexistent/talk-no-egl-vendor.json'}
results = []


def capture(name):
    with tempfile.TemporaryDirectory(prefix='talk-fallback-shot-') as folder:
        subprocess.run(['weston-screenshooter'], env=dict(os.environ, XDG_PICTURES_DIR=folder),
                       check=True, timeout=10, capture_output=True)
        picture = next(Path(folder).glob('*.png'))
        path = args.output / f'{name}.png'
        subprocess.run(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-y', '-i', str(picture),
                        '-vf', 'vflip', str(path)], check=True, timeout=10)
        return path


def record(data):
    path = data / 'graphics.json'
    return json.loads(path.read_text()) if path.exists() else None


def launch(name, data, renderer=None, environment=None, wait=2.0, keep=False):
    command = [str(args.exe.resolve()), '--preview', '--data-dir', str(data)]
    if renderer:
        command += ['--renderer', renderer]
    log_path = args.output / f'{name}.log'
    log = log_path.open('wb')
    process = subprocess.Popen(command, env=dict(os.environ, **(environment or {})),
                               stdout=log, stderr=log, start_new_session=True)
    time.sleep(wait)
    running = process.poll() is None
    picture = capture(name) if running else None
    if running and not keep:
        os.killpg(process.pid, signal.SIGTERM)
        process.wait(timeout=10)
    log.close()
    return process, running, log_path.read_text(), picture


def check(name, condition, detail):
    results.append({'scenario': name, 'passed': bool(condition), 'detail': detail})
    print(('PASS ' if condition else 'FAIL ') + name + ': ' + detail, flush=True)


def opened(log):
    plain = re.sub(r'\x1b\[[0-9;]*m', '', log)
    return [line.split('renderer=')[1].split()[0].strip('"')
            for line in plain.splitlines() if 'starting the Slint desktop window' in line]


base = Path(tempfile.mkdtemp(prefix='talk-fallback-data-', dir=args.output))

# 1. A failed automatic GPU start continues in Skia Software in the same session.
data = base / 'broken-driver'
_, running, log, picture = launch('auto-broken-egl', data, environment=BROKEN_EGL, wait=5.5)
check('broken EGL relaunches with Skia Software',
      running and opened(log) == ['skia-opengl', 'skia-software'] and 'restarting with Skia Software' in log,
      f'running={running} renderers={opened(log)} screenshot={picture}')
saved = record(data)
check('failure is recorded for later starts', saved and saved['state'] == 'failed' and saved['reason'],
      json.dumps(saved))

# 2. The next automatic start skips the GPU and explains why.
_, running, log, picture = launch('auto-after-failure', data)
check('recorded failure starts Skia Software directly',
      running and opened(log) == ['skia-software'], f'renderers={opened(log)} screenshot={picture}')

# 3. Explicit GPU with a broken driver reports the error and does not relaunch.
process, running, log, _ = launch('explicit-gpu-broken-egl', data, renderer='gpu',
                                  environment=BROKEN_EGL, wait=4.5)
check('explicit GPU failure exits without fallback',
      not running and process.returncode != 0 and 'skia-software' not in opened(log),
      f'exit={process.returncode} renderers={opened(log)}')

# 4. A confirmed explicit GPU start clears the recorded failure.
_, running, log, _ = launch('explicit-gpu-recovers', data, renderer='gpu', wait=4.5)
check('confirmed explicit GPU start clears the record',
      running and opened(log) == ['skia-opengl'] and record(data) is None,
      f'renderers={opened(log)} record={record(data)}')

# 5. Automatic mode uses the GPU again and confirms it.
_, running, log, picture = launch('auto-gpu', data, wait=4.5)
check('automatic start uses and confirms Skia GPU',
      running and opened(log) == ['skia-opengl'] and record(data) is None,
      f'renderers={opened(log)} record={record(data)} screenshot={picture}')

# 6. A process that stops during GPU startup selects software next time.
data = base / 'stopped-start'
process, running, log, _ = launch('auto-killed-during-start', data, wait=0.8, keep=True)
os.killpg(process.pid, signal.SIGKILL)
process.wait(timeout=10)
interrupted = record(data)
_, running, log, picture = launch('auto-after-killed-start', data)
check('unconfirmed GPU start selects software next time',
      interrupted and interrupted['state'] == 'starting' and opened(log) == ['skia-software'],
      f'record_after_kill={interrupted} renderers={opened(log)} screenshot={picture}')

(args.output / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
raise SystemExit(0 if all(result['passed'] for result in results) else 1)
