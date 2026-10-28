#!/usr/bin/env python3
"""Local splitscreen dev launcher (temporary, on the splitscreen branch).

    cd dev && uv run launch.py      (or, outside Windows: python3 dev/launch.py)

Up/down picks a row, left/right (or Enter) changes it, L launches, B builds, Q quits.
The game plays from dev/home (its worlds, options and splitscreen profiles), never your
real game folder. Choices are remembered in dev/launcher.json.

Account: P1 can play signed in as one of the Horizon launcher's Microsoft accounts (for
online-mode servers). Its session goes to the game as the launcher hands it over: a private
session file (--session-file) in the game folder, deleted after. The token comes from the
system's keychain (macOS asks once: Always Allow) and lasts 24 h: when it has run out, press
Play once in the real launcher, which renews it.
"""

import curses
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

DEV = Path(__file__).resolve().parent
ROOT = DEV.parent
GAME = ROOT / 'target/fast' / ('horizon-client.exe' if os.name == 'nt' else 'horizon-client')
RESOURCES = ROOT / '.local/horizon-client/26.3'
SAVED = DEV / 'launcher.json'
# The Horizon launcher's own data and keychain service (launcher/src/backend.rs `data_dir`,
# launcher/src/account.rs `SERVICE`): read only.
LAUNCHER = Path(os.environ.get('APPDATA', '')) / 'horizon-launcher' if os.name == 'nt' else (
    Path.home() / ('Library/Application Support' if sys.platform == 'darwin' else '.local/share')
    / 'horizon-launcher')
SERVICE = 'dev.playhorizon.launcher'
OFFLINE = 'offline'


def accounts():
    """The launcher's Microsoft accounts, the one it plays with first: [(name, uuid)]."""
    try:
        data = json.loads((LAUNCHER / 'accounts.json').read_text())
    except (OSError, ValueError):
        return []
    found = [(a['name'], a['uuid']) for a in data.get('accounts', [])]
    return sorted(found, key=lambda a: a[1] != data.get('selected'))


def secrets(uuid):
    """The account's tokens as the launcher keeps them (JSON), from the system's keychain: in
    pieces "<uuid>", "<uuid>.1", "<uuid>.2"... (account.rs `split_secret`), joined."""
    first = keychain_item(uuid)
    if first is None:
        return None
    pieces, n = [first], 1
    while (piece := keychain_item(f'{uuid}.{n}')) is not None:
        pieces.append(piece)
        n += 1
    return ''.join(pieces)


def keychain_item(key):
    """One item of the launcher's service, or None."""
    if sys.platform == 'darwin':
        out = subprocess.run(['security', 'find-generic-password', '-s', SERVICE, '-a', key, '-w'],
                             capture_output=True, text=True)
        # -w ends the secret with a newline of its own.
        return out.stdout[:-1] if out.returncode == 0 else None
    if os.name == 'nt':
        # Windows Credential Manager, as the keyring crate stores it: target "<user>.<service>",
        # the secret UTF-16LE.
        import ctypes
        from ctypes import wintypes

        class CREDENTIAL(ctypes.Structure):
            _fields_ = [('Flags', wintypes.DWORD), ('Type', wintypes.DWORD),
                        ('TargetName', wintypes.LPWSTR), ('Comment', wintypes.LPWSTR),
                        ('LastWritten', wintypes.FILETIME), ('CredentialBlobSize', wintypes.DWORD),
                        ('CredentialBlob', ctypes.POINTER(ctypes.c_char)), ('Persist', wintypes.DWORD),
                        ('AttributeCount', wintypes.DWORD), ('Attributes', ctypes.c_void_p),
                        ('TargetAlias', wintypes.LPWSTR), ('UserName', wintypes.LPWSTR)]
        advapi = ctypes.WinDLL('advapi32')
        cred = ctypes.POINTER(CREDENTIAL)()
        if not advapi.CredReadW(f'{key}.{SERVICE}', 1, 0, ctypes.byref(cred)):
            return None
        try:
            blob = ctypes.string_at(cred.contents.CredentialBlob, cred.contents.CredentialBlobSize)
            return blob.decode('utf-16-le')
        finally:
            advapi.CredFree(cred)
    return None  # ponytail: Linux's Secret Service needs a D-Bus client; add when someone tests there


def session_file(home, name, uuid):
    """The account's session for the game, in a private file in `home` (as the launcher writes
    it); None and why when it can't."""
    text = secrets(uuid)
    if not text:
        return None, 'its tokens are not in this system\'s keychain (or access was refused)'
    s = json.loads(text)
    if s.get('minecraft_expires_at', 0) <= time.time() + 60:
        return None, 'its Minecraft token has run out: press Play once in the Horizon launcher'
    path = home / 'sessions' / 'dev-session.json'
    path.parent.mkdir(parents=True, exist_ok=True)
    path.unlink(missing_ok=True)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'w') as f:
        json.dump({'name': name, 'uuid': uuid, 'access_token': s['minecraft_token'],
                   'user_type': 'msa', 'xuid': s.get('xuid')}, f)
    return path, None


SIZES = ['1280x720', '1600x900', '1920x1080', '2560x1440']
# (key, label, choices); a None list is free text.
ROWS = [
    ('world', 'World', None),
    ('mode', 'Mode', ['creative', 'survival']),
    ('splitscreen', 'Splitscreen setting', ['on', 'off']),
    ('test_players', 'Extra players without controllers', [str(n) for n in range(8)]),
    ('size', 'Window', SIZES),
    ('audio', 'Audio', ['on', 'off']),
    ('home', 'Game folder', ['dev/home', 'fresh (deleted after)']),
    ('build', 'Build before launch', ['on', 'off']),
    ('stats', 'Performance readout (every 5 s)', ['off', 'on']),
    ('framerate', 'Frame rate', ['as set in game', 'uncapped']),
    # The launcher's accounts (filled in by `load`), or offline.
    ('account', 'Account (P1)', [OFFLINE]),
]
DEFAULTS = {'world': 'Splitscreen', 'mode': 'creative', 'splitscreen': 'on', 'test_players': '0',
            'size': '1600x900', 'audio': 'on', 'home': 'dev/home', 'build': 'on',
            'stats': 'off', 'framerate': 'as set in game', 'account': OFFLINE}
# Windows curses sends the keypad's Enter as PADENTER.
ENTER = (10, 13, curses.KEY_ENTER, getattr(curses, 'PADENTER', curses.KEY_ENTER))


def load():
    # The account row: the launcher's accounts, the one it plays with first (the default), then
    # offline. Several accounts: switch here per launch.
    names = [name for name, _ in accounts()]
    ROWS[-1] = ('account', 'Account (P1)', names + [OFFLINE])
    DEFAULTS['account'] = names[0] if names else OFFLINE
    try:
        c = {**DEFAULTS, **json.loads(SAVED.read_text())}
    except (OSError, ValueError):
        return dict(DEFAULTS)
    # A saved choice a row no longer offers goes back to its default.
    for key, _, choices in ROWS:
        if choices and c[key] not in choices:
            c[key] = DEFAULTS[key]
    return c


def set_extra(home, key, value, file='horizon-extras.txt'):
    """One `key:value` line of horizon-extras.txt (or options.txt), keeping the others."""
    path = home / file
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
    if c['framerate'] == 'uncapped':
        # P1's options pace every player in splitscreen. 260 is the slider's Unlimited.
        set_extra(home, 'enableVsync', 'false', 'options.txt')
        set_extra(home, 'maxFps', '260', 'options.txt')
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
    session = None
    if c['account'] != OFFLINE:
        uuid = dict(accounts()).get(c['account'])
        session, why = session_file(home, c['account'], uuid) if uuid else (None, 'not in the launcher')
        if session is None:
            if input(f'{c["account"]} can\'t sign in: {why}.\nPlay offline instead? [y/N] ').lower() != 'y':
                return
        else:
            args += ['--session-file', str(session)]
    print(f'Playing from {home}\n$ {" ".join(args)}\n', flush=True)
    # The game keeps options.txt and saves in its working folder.
    code = subprocess.run(args, cwd=home, env=env).returncode
    if session:
        session.unlink(missing_ok=True)
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
