"""Shared pieces of the voice stress scripts: fixtures, screenshots and counters.

The scripts run inside scripts/run-framework-isolated, with the local stack up.
"""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent
WIDTH, HEIGHT = 1120, 760
TICKS = os.sysconf('SC_CLK_TCK')


def provision():
    """Create a voice-demo fixture: Alice, Bob, a Space, a text and a voice room."""
    output = subprocess.run(['cargo', 'run', '--locked', '--no-default-features', '--', '--voice-demo'],
                            check=True, capture_output=True, text=True, cwd=ROOT).stdout
    for line in output.splitlines():
        if '--data-dir' in line and line.rstrip().endswith('/alice'):
            return ROOT / Path(line.split('--data-dir', 1)[1].strip()).parent
    raise SystemExit('voice demo did not report its accounts:\n' + output)


def capture(folder, name):
    time.sleep(.3)
    with tempfile.TemporaryDirectory(prefix='talk-voice-shot-') as temporary:
        subprocess.run(['weston-screenshooter'], env=dict(os.environ, XDG_PICTURES_DIR=temporary),
                       check=True, timeout=10, capture_output=True)
        picture = next(Path(temporary).glob('*.png'))
        path = Path(folder) / f'{name}.png'
        subprocess.run(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-y', '-i', str(picture),
                        '-vf', 'vflip', str(path)], check=True, timeout=10)
        return path


def locate_rail(picture):
    """Top-left corner of Talk's window content, found by the rail's exact colour."""
    # The rail's exact graphite (Theme.rail, #111215) forms the window's long left edge.
    pixels = subprocess.check_output(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-i', str(picture),
                                      '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-'], timeout=10)
    output_width, output_height = 1600, 1000
    target = bytes([17, 18, 21])
    columns = [sum(pixels[(y * output_width + x) * 3:(y * output_width + x) * 3 + 3] == target
                   for y in range(output_height)) for x in range(output_width)]
    best = max(columns)
    if best < HEIGHT - 80:
        raise RuntimeError(f'Talk rail was not found; inspect {picture}')
    x = next(x for x, count in enumerate(columns) if count >= best - 2)
    ys = [y for y in range(output_height)
          if pixels[(y * output_width + x) * 3:(y * output_width + x) * 3 + 3] == target]
    return x, min(ys)


class Container:
    """CPU and network counters of one test-stack container, read from the host."""

    def __init__(self, name):
        details = json.loads(subprocess.check_output(['docker', 'inspect', name]))[0]
        self.cpu = Path(f'/sys/fs/cgroup/system.slice/docker-{details["Id"]}.scope/cpu.stat')
        self.net = Path(f'/proc/{details["State"]["Pid"]}/net/dev')

    def read(self):
        usage = next(int(line.split()[1]) for line in self.cpu.read_text().splitlines()
                     if line.startswith('usage_usec'))
        fields = next(line.split(':', 1)[1].split() for line in self.net.read_text().splitlines()
                      if line.strip().startswith('eth0:'))
        return {'cpu_seconds': usage / 1e6, 'rx_bytes': int(fields[0]), 'rx_packets': int(fields[1]),
                'tx_bytes': int(fields[8]), 'tx_packets': int(fields[9])}


def process_cpu_seconds(pid):
    fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
    return (int(fields[11]) + int(fields[12])) / TICKS


def process_memory_mib(pid):
    """PSS and RSS from smaps_rollup, in MiB."""
    values = {}
    for line in Path(f'/proc/{pid}/smaps_rollup').read_text().splitlines():
        name, _, rest = line.partition(':')
        if name in ('Rss', 'Pss'):
            values[name.lower()] = int(rest.split()[0]) / 1024
    return values


def host_ticks():
    values = [int(value) for value in Path('/proc/stat').read_text().splitlines()[0].split()[1:]]
    return sum(values), values[3] + values[4]
