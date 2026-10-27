#!/usr/bin/env python3
"""Splitscreen end to end: a real game with players added by HORIZON_SPLIT, driven through each
player's control socket, checking every player's view size after each change.

    python3 tools/splitscreen_e2e.py
    python3 tools/splitscreen_e2e.py --only windows    # one scenario: join, windows, three, live, picker

Scenarios:
  join     four players fill Window 1's quarters; one disconnects and the others take thirds
  windows  a player opens a New window (Window 2), another shares it (halves in each window), the
           first leaves it again
  three    three in a window: one wide on top, one tall on the left, side by side
  live     splitscreen off and on again from the Splitscreen page, in a world
  picker   a joining player makes a profile, picks New window, and joins in it
  bumpers  a joining player moves its view with LB and RB before picking its profile
  eight    eight players in one window: rows of three, three and two, every view drawing
  fullscreen  a player whose options say fullscreen joins: the window stays as P1 has it; its F11
              in a window of its own toggles that window only

Needs a desktop session (the windows open on screen) and the vanilla data in .local/. Each game
gets fresh scratch folders (lab.game_env) and runs from inside them. Exit status 0 when every check
passed.
"""

import argparse
import json
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

try:
    from . import lab
except ImportError:
    import lab

ROOT = Path(__file__).resolve().parents[1]
CTL = lab.cargo_target() / 'fast/horizon-ctl'
RESOURCES = ROOT / '.local/horizon-client/26.3'
# Logical; the views are checked as fractions of what P1 gets alone, so any scale works.
WIDTH, HEIGHT = 1000, 600

failures = []


def check(name, got, want):
    ok = got == want
    print(f'  {"ok  " if ok else "FAIL"} {name}: {got}' + ('' if ok else f', expected {want}'))
    if not ok:
        failures.append(name)


class Game:
    """One splitscreen session: P1 launched with `players` (HORIZON_SPLIT), each on its socket."""

    def __init__(self, players, pick=False, seed=None):
        self.home = Path(tempfile.mkdtemp(prefix='horizon-split-e2e-'))
        if seed:
            seed(self.home)
        self.sockets = Path(tempfile.mkdtemp(prefix='hzse-'))  # short: Unix socket paths
        extra = {'HORIZON_SPLIT': str(players)}
        if pick:
            extra['HORIZON_SPLIT_PICK'] = '1'
        self.env = lab.game_env(HORIZON_HOME=self.home, **extra)
        self.players = players
        self.ctl(1, 'launch', '--wait', '--world', 'Split', '--creative', '--resources', str(RESOURCES),
                 '--width', str(WIDTH), '--height', str(HEIGHT), '--render-distance', '4',
                 # Another scenario's windows closing take the focus: no pause on losing it.
                 '--no-pause')
        self.wait(lambda: all(self.alive(n) for n in range(1, players + 1)), 'every player in')
        # The automatic GUI scale keeps menus about 400 x 240: room for every row of the page.
        self.ctl(1, 'options-set', 'guiScale', '2')

    def ctl(self, n, *words):
        socket = self.sockets / ('c.sock' if n == 1 else f'c.sock{n}')
        out = subprocess.run([str(CTL), '--socket', str(socket), *words], env=self.env,
                             cwd=self.home, capture_output=True, text=True)
        reply = json.loads(out.stdout or '{}')
        if 'error' in reply:
            raise RuntimeError(f'P{n} {" ".join(words)}: {reply["error"]}')
        return reply

    def alive(self, n):
        try:
            self.ctl(n, 'status')
            return True
        except (RuntimeError, json.JSONDecodeError):
            return False

    def size(self, n):
        return tuple(self.ctl(n, 'status')['renderer']['size'])

    def sizes(self, players):
        return [self.size(n) for n in players]

    @staticmethod
    def wait(done, what, seconds=30):
        end = time.time() + seconds
        while time.time() < end:
            if done():
                return
            time.sleep(0.3)
        raise RuntimeError(f'timed out waiting for {what}')

    def settle(self, players, want):
        """Wait until the players' views have the sizes `want` (a moment for windows to open)."""
        try:
            self.wait(lambda: self.sizes(players) == want, 'the layout', seconds=8)
        except RuntimeError:
            pass
        return self.sizes(players)

    def labels(self, n=1):
        return [w.get('label', '') for w in self.ctl(n, 'ui')['widgets']]

    def page(self, n=1):
        """Open player n's Splitscreen page (once it is drawn); its widgets."""
        if any(l.startswith('Splitscreen: ') for l in self.labels(n)):
            return self.ctl(n, 'ui')['widgets']  # still open (Esc taken during a rebuild)
        self.ctl(n, 'options', 'horizon')
        try:
            self.wait(lambda: 'Splitscreen...' in self.labels(n), 'Horizon Settings', seconds=15)
        except RuntimeError:
            ui = self.ctl(n, 'ui')
            raise RuntimeError(f'no Horizon Settings: screen {ui.get("screen")}, '
                               f'buttons {[w["label"] for w in ui["widgets"]]}') from None
        self.ctl(n, 'click-button', 'Splitscreen...')
        self.wait(lambda: any(l.startswith('Splitscreen: ') for l in self.labels(n)),
                  'the Splitscreen page', seconds=5)
        return self.ctl(n, 'ui')['widgets']

    def screens(self, by=1):
        """Every player's Play on, as player `by`'s page shows them."""
        return [l for l in self.labels(by) if l.startswith('Play on:')]

    def play_on(self, player, where, by=1):
        """Click player `player`'s Play on (forward: buttons take the left click only) until it
        reads `where`, on player `by`'s Splitscreen page."""
        self.page(by)
        for _ in range(6):
            if self.screens(by)[player - 1] == f'Play on: {where}':
                break
            row = [w for w in self.ctl(by, 'ui')['widgets'] if w.get('label', '').startswith('Play on:')]
            self.ctl(by, 'click', row[player - 1]['id'])
            time.sleep(0.3)
        check(f'P{player} plays on {where}', self.screens(by)[player - 1], f'Play on: {where}')
        self.close_page(by)

    def close_page(self, n=1):
        """Back to playing: Esc through the options and the pause menu."""
        for _ in range(6):
            if self.ctl(n, 'ui').get('screen') not in ('options', 'paused'):
                return
            self.ctl(n, 'key', 'esc', 'tap')
            time.sleep(0.5)

    def quit(self):
        """Save and quit, and wait until the game is gone (the next scenario starts clean)."""
        try:
            self.ctl(1, 'quit')
            self.wait(lambda: not self.alive(1), 'the game to quit')
        except (RuntimeError, json.JSONDecodeError):
            pass
        shutil.rmtree(self.sockets, ignore_errors=True)


def scenario_join():
    print('join: four players, then one leaves')
    g = Game(4)
    try:
        quarters = g.sizes([1, 2, 3, 4])
        check('four take equal quarters', len(set(quarters)), 1)
        w, h = quarters[0]
        g.ctl(4, 'key', 'esc', 'tap')
        g.wait(lambda: 'Leave' in g.labels(4), "P4's pause menu", seconds=5)
        check("P4's pause menu says Leave", 'Leave' in g.labels(4), True)
        g.ctl(4, 'disconnect')
        three = g.settle([1, 2, 3], [(2 * w, h), (w, h), (w, h)])
        check('three: one wide on top, two below', three, [(2 * w, h), (w, h), (w, h)])
    finally:
        g.quit()


def scenario_windows():
    print('windows: New window, share it, leave it')
    g = Game(4)
    try:
        w, h = g.size(1)
        g.play_on(4, 'Window 2')  # New window: the first free number
        main = g.settle([1, 2, 3], [(2 * w, h), (w, h), (w, h)])
        check('Window 1: one wide on top, two below', main, [(2 * w, h), (w, h), (w, h)])
        alone = g.size(4)
        g.play_on(3, 'Window 2')  # P4's
        halves = g.settle([1, 2], [(w, 2 * h), (w, 2 * h)])
        check('Window 1: P1 and P2 side by side', halves, [(w, 2 * h), (w, 2 * h)])
        shared = g.sizes([3, 4])
        check('Window 2: P3 and P4 share it', shared[0] == shared[1] and shared[0][0] < alone[0], True)
        g.play_on(3, 'Window 1')
        back = g.settle([1, 2, 3], [(2 * w, h), (w, h), (w, h)])
        check('P3 back in Window 1', back, [(2 * w, h), (w, h), (w, h)])
        check('P4 alone in Window 2 again', g.size(4), alone)
    finally:
        g.quit()


def scenario_three():
    print('three: the three layouts')
    g = Game(3)
    try:
        w2, h = g.size(2)  # one wide on top by default: P2 has a quarter
        layouts = [
            ('one tall on the left', [(w2, 2 * h), (w2, h), (w2, h)]),
            ('side by side', None),
            ('one wide on top', [(2 * w2, h), (w2, h), (w2, h)]),
        ]
        for name, want in layouts:
            g.page()
            g.ctl(1, 'click-button', 'Three on a screen')
            g.close_page()
            got = g.settle([1, 2, 3], want) if want else g.settle([1, 2, 3], [g.size(1)] * 3)
            if want:
                check(name, got, want)
            else:
                widths = [x for x, _ in got]
                check(f'{name}: three columns, full height',
                      max(widths) - min(widths) <= 1 and all(y == 2 * h for _, y in got), True)
    finally:
        g.quit()


def scenario_live():
    print('live: splitscreen off and on in a world')
    g = Game(2)
    try:
        half = g.size(1)
        g.page()
        g.ctl(1, 'click-button', 'Splitscreen: On')
        g.close_page()
        g.wait(lambda: not g.alive(2), 'P2 gone')
        full = g.settle([1], [(2 * half[0], half[1])])
        check('off: P1 has the whole window', full, [(2 * half[0], half[1])])
        g.page()
        g.ctl(1, 'click-button', 'Splitscreen: Off')
        g.close_page()
        time.sleep(1)
        status = g.ctl(1, 'status')
        check('on again: P1 still renders the whole window', tuple(status['renderer']['size']),
              (2 * half[0], half[1]))
        check('on again: frames keep coming', status['fps'] > 10, True)
    finally:
        g.quit()


def scenario_picker():
    print('picker: a new profile in a New window')
    g = Game(2, pick=True)
    try:
        g.wait(lambda: (g.ctl(2, 'state').get('menu') or {}).get('mode') == 'splitscreen-profiles',
               'the picker')
        half = g.size(1)  # P2 picks in Window 1 beside P1
        g.ctl(2, 'click-button', 'Play on')  # As last time -> Window 1
        g.ctl(2, 'click-button', 'Play on')  # -> New window
        g.ctl(2, 'click-button', 'New Profile...')
        g.ctl(2, 'type', 'Sam')
        g.ctl(2, 'click-button', 'Create')
        g.wait(lambda: g.ctl(2, 'state').get('player') is not None, 'Sam in the world')
        check('P1 has Window 1 to itself', g.settle([1], [(2 * half[0], half[1])]), [(2 * half[0], half[1])])
        check('Sam plays in its own window', g.size(2) != g.size(1), True)
        saved = (g.home / 'splitscreen/profiles/Sam/screen.txt').read_text().strip()
        check('the profile remembers a window', saved, 'window')
    finally:
        g.quit()


def scenario_fullscreen():
    print('fullscreen: a profile that was fullscreen joins')

    def seed(home):
        profile = home / 'splitscreen/profiles/P2'
        profile.mkdir(parents=True)
        (profile / 'options.txt').write_text('fullscreen:true\n')

    g = Game(2, seed=seed)
    try:
        time.sleep(3)
        value = g.ctl(1, 'options-get', 'fullscreen')['options']['fullscreen']['value']
        check("P1's window stays windowed (P1's fullscreen option)", value, False)
        # P2's F11 in a window of its own: that window, and only it, goes fullscreen and back.
        g.play_on(2, 'Window 2')
        windowed = g.size(2)
        g.ctl(2, 'key', 'f11', 'tap')
        g.wait(lambda: g.size(2)[0] > windowed[0], "P2's window fullscreen", seconds=10)
        check("P2's F11: its window fills the screen", g.size(2)[0] > windowed[0], True)
        value = g.ctl(1, 'options-get', 'fullscreen')['options']['fullscreen']['value']
        check("P1's window stays windowed", value, False)
        full = g.size(2)
        time.sleep(2)  # macOS ignores a change while its fullscreen animation runs
        g.ctl(2, 'key', 'f11', 'tap')
        try:
            g.wait(lambda: g.size(2) == windowed, "P2's window back", seconds=10)
        except RuntimeError:
            print(f'    (windowed {windowed}, fullscreen {full}, now {g.size(2)})')
        check("P2's F11 again: back to its window", g.size(2), windowed)
    finally:
        g.quit()


def scenario_bumpers():
    print('bumpers: a joining player moves its view with LB and RB, then picks a profile')
    g = Game(2, pick=True)
    try:
        g.wait(lambda: (g.ctl(2, 'state').get('menu') or {}).get('mode') == 'splitscreen-profiles',
               'the picker')
        half = g.size(1)
        whole = (2 * half[0], half[1])
        g.ctl(2, 'pad', 'rb', 'tap')
        check('RB: P2 opens Window 2, P1 has Window 1 to itself', g.settle([1], [whole]), [whole])
        own = g.size(2)
        check("P2's picker shows it", 'Play on: New window' in g.labels(2), True)
        g.ctl(2, 'pad', 'lb', 'tap')
        check('LB: back beside P1 in Window 1', g.settle([1, 2], [half, half]), [half, half])
        g.ctl(2, 'pad', 'rb', 'tap')
        g.settle([2], [own])
        g.ctl(2, 'click-button', 'New Profile...')
        g.ctl(2, 'type', 'Ann')
        g.ctl(2, 'click-button', 'Create')
        g.wait(lambda: g.ctl(2, 'state').get('player') is not None, 'Ann in the world')
        check('Ann joins where the bumpers put her', g.settle([1, 2], [whole, own]), [whole, own])
        saved = (g.home / 'splitscreen/profiles/Ann/screen.txt').read_text().strip()
        check('her profile remembers a window', saved, 'window')
    finally:
        g.quit()


def scenario_eight():
    print('eight: eight players in one window')
    g = Game(8)
    try:
        sizes = g.sizes(range(1, 9))
        near = lambda a, b: abs(a - b) <= 1  # thirds of an odd width differ by a pixel
        # Rows of three, three and two: six equal views, then two wider ones; all the same height.
        check(f'rows of 3, 3 and 2: {sizes}',
              all(near(x[0], sizes[0][0]) and near(x[1], sizes[0][1]) for x in sizes[:6])
              and near(sizes[6][0], sizes[7][0]) and sizes[6][0] > sizes[0][0] + 100
              and all(near(x[1], sizes[0][1]) for x in sizes), True)
        drawing = lambda: all(g.ctl(n, 'status')['fps'] > 10 for n in range(1, 9))
        try:
            g.wait(drawing, 'every view drawing', seconds=20)
        except RuntimeError:
            pass
        check('every view draws', drawing(), True)
    finally:
        g.quit()


SCENARIOS = {
    'join': scenario_join,
    'windows': scenario_windows,
    'three': scenario_three,
    'live': scenario_live,
    'picker': scenario_picker,
    'fullscreen': scenario_fullscreen,
    'eight': scenario_eight,
    'bumpers': scenario_bumpers,
}


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--only', choices=SCENARIOS)
    args = ap.parse_args()
    if not CTL.exists():
        sys.exit(f'{CTL} is missing: cargo build --profile fast -p horizon-client')
    if not (RESOURCES / 'assets').exists():
        sys.exit(f'no prepared resources in {RESOURCES}')
    for name, run in SCENARIOS.items():
        if args.only in (None, name):
            try:
                run()
            except RuntimeError as e:
                print(f'  FAIL {name}: {e}')
                failures.append(name)
    if failures:
        sys.exit(f'{len(failures)} check(s) failed: {", ".join(failures)}')
    print('splitscreen: every check passed')


if __name__ == '__main__':
    main()
