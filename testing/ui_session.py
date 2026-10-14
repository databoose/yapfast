#!/usr/bin/env python3
"""Run Talk on the private display and act on commands from scripts/ui-session.

scripts/ui-session starts this inside scripts/run-framework-isolated. It
starts Talk (and, with --discord, testing/fake_discord.ts), then reads
"ID<TAB>COMMAND" lines appended to STATE/commands and answers each with
"ID<TAB>ok|error<TAB>JSON_OUTPUT" in STATE/results. See scripts/ui-session for the
commands.
"""
import argparse
import json
import os
from pathlib import Path
import runpy
import shlex
import signal
import socket
import subprocess
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
INPUT = runpy.run_path(str(ROOT / 'scripts/framework-input'))
INPUT['require_private_display']()

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--state', type=Path, required=True)
parser.add_argument('--bin', type=Path, required=True)
parser.add_argument('--size', default='1120x760')
parser.add_argument('--renderer', choices=['software', 'gpu'], default='software')
parser.add_argument('--discord', action='store_true')
parser.add_argument('--fake-args', default='')
parser.add_argument('--tray', type=Path, help='testing/tray_host.rs, built, to run as the tray')
parser.add_argument('talk_args', nargs=argparse.REMAINDER)
args = parser.parse_args()
talk_args = args.talk_args[1:] if args.talk_args[:1] == ['--'] else args.talk_args
state = args.state

KEYS = {
    'esc': 1, 'backspace': 14, 'tab': 15, 'enter': 28, 'ctrl': 29, 'shift': 42,
    'alt': 56, 'space': 57, 'home': 102, 'up': 103, 'pageup': 104, 'left': 105,
    'right': 106, 'end': 107, 'down': 108, 'pagedown': 109, 'delete': 111,
    'f10': 68, 'menu': 127,
}
KEYS.update(zip('abcdefghijklmnopqrstuvwxyz',
                [30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44]))
KEYS.update(zip('1234567890', range(2, 12)))
# Characters `type` can enter on the US layout, and whether they need Shift.
CHARACTERS = {character: (code, False) for character, code in KEYS.items() if len(character) == 1}
CHARACTERS.update({character.upper(): (code, True) for character, code in KEYS.items()
                   if len(character) == 1 and character.isalpha()})
CHARACTERS.update({' ': (57, False), '-': (12, False), '=': (13, False), ';': (39, False),
                   "'": (40, False), ',': (51, False), '.': (52, False), '/': (53, False),
                   '!': (2, True), '@': (3, True), '#': (4, True), '_': (12, True),
                   ':': (39, True), '?': (53, True), '(': (10, True), ')': (11, True)})

fake = None
# Talk reads commands for its window from this file; see `test_control` in src/ui.rs.
window_control = state / 'window-control'
environment = dict(os.environ, TALK_UI_TEST_SIZE=args.size, TALK_UI_TEST_CONTROL=str(window_control))
if args.discord:
    with socket.socket() as probe:
        probe.bind(('127.0.0.1', 0))
        port = probe.getsockname()[1]
    discord = f'http://127.0.0.1:{port}'
    fake = subprocess.Popen(['bun', str(ROOT / 'testing/fake_discord.ts'), '--port', str(port),
                             *shlex.split(args.fake_args)],
                            stdout=(state / 'fake.log').open('wb'), stderr=subprocess.STDOUT,
                            start_new_session=True)
    deadline = time.monotonic() + 20
    while True:
        try:
            urllib.request.urlopen(f'{discord}/_control/health', timeout=1)
            break
        except OSError:
            if fake.poll() is not None or time.monotonic() > deadline:
                raise SystemExit('The fake Discord did not start; see ' + str(state / 'fake.log'))
            time.sleep(.2)
    environment['TALK_DISCORD_TEST_SERVER'] = discord

data = state / 'data'
talk = None
tray = None


def start_tray():
    """The stand-in tray, on this session's private bus, as testing/tray_host.rs says."""
    global tray
    tray = subprocess.Popen([str(args.tray), 'watch'], stdout=(state / 'tray.log').open('ab'),
                            stderr=subprocess.STDOUT, start_new_session=True)
    time.sleep(.5)
    if tray.poll() is not None:
        raise RuntimeError(f'The tray exited with {tray.returncode}; see {state / "tray.log"}')


def tray_command(rest):
    if args.tray is None:
        raise ValueError('tray needs start --tray')
    if rest == 'off':
        stop(tray)
        return ''
    if rest == 'on':
        if tray is None or tray.poll() is not None:
            start_tray()
        return ''
    values = rest.split(' ', 1)
    if values[0] == 'icon':
        shots = state / 'shots'
        shots.mkdir(exist_ok=True)
        picture = shots / f'{values[1]}.pam'
        size = subprocess.run([str(args.tray), 'icon', str(picture)], check=True, capture_output=True,
                              text=True, timeout=10).stdout.strip()
        path = shots / f'{values[1]}.png'
        subprocess.run(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-y', '-i', str(picture), str(path)],
                       check=True, timeout=10)
        picture.unlink()
        return f'{path} {size}'
    result = subprocess.run([str(args.tray), *values], capture_output=True, text=True, timeout=10)
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip() or f'tray_host exited with {result.returncode}')
    # The window opens, or the menu acts, as Talk hears it.
    time.sleep(.8)
    return result.stdout.strip()


def launch():
    global talk
    # A new Talk would run every line again.
    window_control.write_text('')
    # Talk takes --preview only before --data-dir.
    talk = subprocess.Popen([str(args.bin), '--renderer', args.renderer, *talk_args, '--data-dir', str(data)],
                            env=environment, stdout=(state / 'talk.log').open('ab'),
                            stderr=subprocess.STDOUT, start_new_session=True)
    (state / 'talk.pid').write_text(f'{talk.pid}\n')
    time.sleep(2)
    if talk.poll() is not None:
        raise RuntimeError(f'Talk exited with {talk.returncode}; see {state / "talk.log"}')


def stop(process):
    if process is None or process.poll() is not None:
        return
    os.killpg(process.pid, signal.SIGTERM)
    try:
        process.wait(5)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(5)


def shot(name, *crop):
    shots = state / 'shots'
    shots.mkdir(exist_ok=True)
    path = shots / f'{name}.png'
    time.sleep(.4)
    with tempfile.TemporaryDirectory(prefix='talk-shot-') as folder:
        subprocess.run(['weston-screenshooter'], env=dict(os.environ, XDG_PICTURES_DIR=folder),
                       check=True, timeout=10, capture_output=True)
        picture = next(Path(folder).glob('*.png'))
        # The GL renderer's capture is upside down.
        flip = 'vflip'
        if crop:
            x, y, width, height = map(int, crop)
            flip += f',crop={width}:{height}:{x}:{y}'
        subprocess.run(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-y', '-i', str(picture),
                        '-vf', flip, str(path)], check=True, timeout=10)
    return str(path)


def press(name):
    """A key by name or evdev code, with modifiers joined by +, as ctrl+a."""
    *modifiers, key = name.split('+')
    codes = [KEYS[modifier] for modifier in modifiers]
    for code in codes:
        INPUT['send'](f'key {code} 1')
    INPUT['key'](int(key) if key.isdecimal() else KEYS[key])
    for code in reversed(codes):
        INPUT['send'](f'key {code} 0')


def type_text(text):
    for character in text:
        if character not in CHARACTERS:
            raise ValueError(f'type cannot enter {character!r}; paste it or use key')
        code, shift = CHARACTERS[character]
        if shift:
            INPUT['send']('key 42 1')
        INPUT['key'](code)
        if shift:
            INPUT['send']('key 42 0')


def control(method, name):
    if fake is None:
        raise ValueError('control and get need --discord')
    request = urllib.request.Request(f'{discord}/_control/{name}', method=method)
    with urllib.request.urlopen(request, timeout=10) as response:
        return response.read().decode(errors='replace').strip()


def perform(command):
    verb, _, rest = command.partition(' ')
    values = rest.split()
    if verb == 'relaunch':
        stop(talk)
        launch()
        return f'pid {talk.pid}'
    if verb == 'pid':
        return str(talk.pid)
    if verb == 'sleep':
        time.sleep(float(rest))
        return ''
    if verb == 'control':
        return control('POST', rest)
    if verb == 'get':
        return control('GET', rest)
    if verb == 'tray':
        return tray_command(rest)
    if talk.poll() is not None:
        raise RuntimeError(f'Talk exited with {talk.returncode}; see {state / "talk.log"}, or relaunch')
    if verb == 'click':
        # A third word holds modifiers down for the click, as shift or ctrl+shift.
        codes = [KEYS[modifier] for modifier in values[2].split('+')] if len(values) > 2 else []
        for code in codes:
            INPUT['send'](f'key {code} 1')
        INPUT['click'](*map(float, values[:2]))
        for code in reversed(codes):
            INPUT['send'](f'key {code} 0')
        time.sleep(.3)
    elif verb == 'rightclick':
        INPUT['send'](f'move {values[0]} {values[1]}')
        INPUT['send']('button 273 1')
        INPUT['send']('button 273 0')
        time.sleep(.3)
    elif verb == 'move':
        INPUT['send'](f'move {values[0]} {values[1]}')
        time.sleep(.2)
    elif verb == 'drag':
        # Pressed at the first point and moved in steps, as a hand would,
        # then released at the second.
        x, y, to_x, to_y = map(float, values)
        INPUT['send'](f'move {x} {y}')
        INPUT['send']('button 272 1')
        time.sleep(.1)
        for step in range(1, 11):
            INPUT['send'](f'move {x + (to_x - x) * step / 10} {y + (to_y - y) * step / 10}')
            time.sleep(.02)
        INPUT['send']('button 272 0')
        time.sleep(.3)
    elif verb == 'press':
        # Held until `release`, for what happens while a button is down,
        # such as a window being dragged by its title bar.
        x, y = map(float, values)
        INPUT['send'](f'move {x} {y}')
        INPUT['send']('button 272 1')
        time.sleep(.1)
    elif verb == 'release':
        INPUT['send']('button 272 0')
        time.sleep(.3)
    elif verb == 'scroll':
        INPUT['scroll'](float(rest))
        time.sleep(.1)
    elif verb == 'key':
        press(rest)
        time.sleep(.2)
    elif verb == 'type':
        type_text(rest)
        time.sleep(.2)
    elif verb == 'shot':
        return shot(*values)
    elif verb == 'pick':
        kind, _, rest = rest.partition(' ')
        path = (ROOT / rest).resolve()
        if kind not in ('avatar', 'banner') or not path.is_file():
            raise ValueError(f'pick avatar|banner PATH, with a file at {path}')
        with window_control.open('a') as window:
            window.write(f'pick {kind} {path}\n')
        time.sleep(.8)
    elif verb == 'stage':
        path = (ROOT / rest).resolve()
        if not path.is_file():
            raise ValueError(f'no file at {path}')
        with window_control.open('a') as window:
            window.write(f'stage {path}\n')
        # Talk reads the file every half second.
        time.sleep(.8)
    elif verb == 'theme':
        with window_control.open('a') as window:
            window.write(f'theme {rest}\n')
        time.sleep(.8)
    elif verb == 'window':
        if rest not in ('close', 'open', 'destroy', 'create', 'hide', 'show', 'trim', 'stats'):
            raise ValueError(f'unknown window command {rest!r}')
        with window_control.open('a') as window:
            window.write(f'{rest}\n')
        time.sleep(1.2)
    else:
        raise ValueError(f'unknown command {verb!r}')
    return ''


commands = state / 'commands'
results = state / 'results'
try:
    if args.tray:
        start_tray()
    launch()
    (state / 'ready').write_text('ready\n')
    seen = 0
    while True:
        lines = commands.read_text().splitlines() if commands.exists() else []
        for line in lines[seen:]:
            seen += 1
            identifier, _, command = line.partition('\t')
            if command == 'quit':
                with results.open('a') as answers:
                    answers.write(f'{identifier}\tok\t""\n')
                raise SystemExit(0)
            try:
                outcome = 'ok', perform(command)
            except Exception as error:  # Report it to the caller and keep the session.
                outcome = 'error', f'{type(error).__name__}: {error}'
            with results.open('a') as answers:
                answers.write(f'{identifier}\t{outcome[0]}\t{json.dumps(outcome[1])}\n')
        time.sleep(.05)
finally:
    stop(talk)
    stop(tray)
    stop(fake)
