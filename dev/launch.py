#!/usr/bin/env python3
"""Local splitscreen dev launcher (temporary, on the splitscreen branch).

    cd dev && uv run launch.py      (or, outside Windows: python3 dev/launch.py)

Up/down picks a row, left/right (or Enter) changes it, L launches, B builds, Q quits.
The game plays from dev/home (its worlds, options and splitscreen profiles), never your
real game folder. Choices are remembered in dev/launcher.json.
"""

import curses
import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

DEV = Path(__file__).resolve().parent
ROOT = DEV.parent
GAME = ROOT / 'target/fast' / ('horizon-client.exe' if os.name == 'nt' else 'horizon-client')
RESOURCES = ROOT / '.local/horizon-client/26.3'
SAVED = DEV / 'launcher.json'

SIZES = ['1280x720', '1600x900', '1920x1080', '2560x1440']
# (key, label, choices); a None list is free text.
ROWS = [
    ('world', 'World', None),
    ('mode', 'Mode', ['creative', 'survival']),
    ('splitscreen', 'Splitscreen setting', ['on', 'off']),
    ('test_players', 'Extra players without controllers', ['0', '1', '2', '3']),
    ('size', 'Window', SIZES),
    ('audio', 'Audio', ['on', 'off']),
    ('home', 'Game folder', ['dev/home', 'fresh (deleted after)']),
    ('build', 'Build before launch', ['on', 'off']),
    ('stats', 'Performance readout (every 5 s)', ['off', 'on']),
]
DEFAULTS = {'world': 'Splitscreen', 'mode': 'creative', 'splitscreen': 'on', 'test_players': '0',
            'size': '1600x900', 'audio': 'on', 'home': 'dev/home', 'build': 'on',
            'stats': 'off'}
# Windows curses sends the keypad's Enter as PADENTER.
ENTER = (10, 13, curses.KEY_ENTER, getattr(curses, 'PADENTER', curses.KEY_ENTER))


def load():
    try:
        c = {**DEFAULTS, **json.loads(SAVED.read_text())}
    except (OSError, ValueError):
        return dict(DEFAULTS)
    # A saved choice a row no longer offers goes back to its default.
    for key, _, choices in ROWS:
        if choices and c[key] not in choices:
            c[key] = DEFAULTS[key]
    return c


def set_extra(home, key, value):
    """One `key:value` line of horizon-extras.txt, keeping the others."""
    path = home / 'horizon-extras.txt'
    lines = path.read_text().splitlines() if path.exists() else []
    lines = [l for l in lines if not l.startswith(f'{key}:')] + [f'{key}:{value}']
    path.write_text('\n'.join(lines) + '\n')


def build():
    return subprocess.run(['cargo', 'build', '--profile', 'fast', '-p', 'horizon-client'], cwd=ROOT).returncode == 0


def launch(c):
    if c['build'] == 'on' and not build():
        input('Build failed. Enter to go back. ')
        return
    if not GAME.exists():
        input(f'No game at {GAME}: turn on "Build before launch" or press B. Enter to go back. ')
        return
    if not (RESOURCES / 'assets').exists():
        input(f'No prepared resources in {RESOURCES}. From the repo root, with Minecraft 26.3 played once\n'
              '  cargo run --release -p horizon-prepare -- --prepare --server-jar <26.3 server.jar>   (JDK 25)\n'
              '  cargo run --release -p horizon-prepare -- sounds\nEnter to go back. ')
        return
    fresh = c['home'] != 'dev/home'
    home = Path(tempfile.mkdtemp(prefix='horizon-dev-')) if fresh else DEV / 'home'
    home.mkdir(parents=True, exist_ok=True)
    set_extra(home, 'splitscreen', 'true' if c['splitscreen'] == 'on' else 'false')
    env = dict(os.environ)
    env.update(HORIZON_HOME=str(home), HORIZON_LAUNCHER_HOME=str(home / '.launcher'),
               XDG_DATA_HOME=str(home / '.xdg-data'), XDG_CONFIG_HOME=str(home / '.xdg-config'),
               HORIZON_LAUNCHER_NO_SELF_UPDATE='1')
    env.pop('HORIZON_SPLIT', None)
    if c['test_players'] != '0':
        env['HORIZON_SPLIT'] = str(int(c['test_players']) + 1)
    if c['stats'] == 'on':
        env['HORIZON_SPLIT_STATS'] = '1'
    if c['audio'] == 'off':
        env['HORIZON_NO_AUDIO'] = '1'
    width, height = c['size'].split('x')
    args = [str(GAME), '--resources', str(RESOURCES), '--world', c['world'], '--width', width, '--height', height]
    if c['mode'] == 'creative':
        args.append('--creative')
    print(f'Playing from {home}\n$ {" ".join(args)}\n', flush=True)
    # The game keeps options.txt and saves in its working folder.
    code = subprocess.run(args, cwd=home, env=env).returncode
    if fresh:
        shutil.rmtree(home, ignore_errors=True)
    if code != 0:
        input(f'The game exited with {code}. Enter to go back. ')


def tui(screen, c):
    curses.curs_set(0)
    row = 0
    while True:
        screen.erase()
        screen.addstr(1, 2, 'Horizon splitscreen (dev)', curses.A_BOLD)
        for i, (key, label, choices) in enumerate(ROWS):
            mark = curses.A_REVERSE if i == row else curses.A_NORMAL
            value = c[key] if choices is None else f'< {c[key]} >'
            screen.addstr(3 + i, 4, f'{label:<36}', mark)
            screen.addstr(3 + i, 41, value)
        screen.addstr(4 + len(ROWS), 2, 'L launch   B build   Q quit   arrows/Enter change')
        key = screen.getch()
        name, _, choices = ROWS[row]
        if key in (ord('q'), ord('Q'), 27):
            return None
        if key in (ord('l'), ord('L')):
            return 'launch'
        if key in (ord('b'), ord('B')):
            return 'build'
        if key == curses.KEY_UP:
            row = (row - 1) % len(ROWS)
        elif key == curses.KEY_DOWN:
            row = (row + 1) % len(ROWS)
        elif choices is None and key in (*ENTER, curses.KEY_RIGHT):
            curses.echo()
            curses.curs_set(1)
            screen.addstr(3 + row, 41, ' ' * 30)
            text = screen.getstr(3 + row, 41, 30).decode().strip()
            curses.noecho()
            curses.curs_set(0)
            if text:
                c[name] = text
        elif choices and key in (curses.KEY_RIGHT, curses.KEY_LEFT, *ENTER):
            step = -1 if key == curses.KEY_LEFT else 1
            c[name] = choices[(choices.index(c[name]) + step) % len(choices)]
        SAVED.write_text(json.dumps(c, indent=2))


def main():
    c = load()
    while True:
        action = curses.wrapper(tui, c)
        if action is None:
            return
        if action == 'build':
            build()
            input('Enter to go back. ')
        else:
            launch(c)


if __name__ == '__main__':
    main()
