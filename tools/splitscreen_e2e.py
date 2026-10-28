#!/usr/bin/env python3
"""Splitscreen end to end: a real game with players added by HORIZON_SPLIT, driven through each
player's control socket, checking every player's view size after each change.

    python3 tools/splitscreen_e2e.py
    python3 tools/splitscreen_e2e.py --only three eight    # some scenarios (see --help)

Scenarios:
  join     four players fill Window 1's quarters; one disconnects and the others take thirds
  windows  a player opens a New window (Window 2), another shares it (halves in each window), the
           first leaves it again
  three    three in a window: one wide on top, one tall on the left, side by side
  live     splitscreen off and on again from the Splitscreen page, in a world
  picker   a joining player opens a new window (RB), makes a profile, and joins in it
  setup    the Setup screen: a draft moves nobody, Apply moves, Revert undoes, Done closes
  monitor  a player fills another monitor and another joins it (skipped with one monitor)
  signedin P1 signed into an account: another player still joins, under its own profile
  leave    a player holds Select, then A leaves, or X switches profile in the same place
  bumpers  a joining player moves its view with LB and RB before picking its profile
  eight    eight players in one window: rows of three, three and two, every view drawing
  fullscreen  a player whose options say fullscreen joins: the window stays as P1 has it; its F11
              in a window of its own toggles that window only

Needs a desktop session (the windows open on screen) and the vanilla data in .local/. Each game
gets fresh scratch folders (lab.game_env) and runs from inside them. Exit status 0 when every check
passed.
"""

import argparse
import contextlib
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

    def __init__(self, players, pick=False, seed=None, args=(), ready=True):
        """`ready=False`: return once P1's game answers, before it is in a world (it may be asked
        something first); the caller waits for the players."""
        self.home = Path(tempfile.mkdtemp(prefix='horizon-split-e2e-'))
        if seed:
            seed(self.home)
        self.sockets = Path(tempfile.mkdtemp(prefix='hzse-'))  # short: Unix socket paths
        extra = {'HORIZON_SPLIT': str(players)}
        if pick:
            extra['HORIZON_SPLIT_PICK'] = '1'
        self.env = lab.game_env(HORIZON_HOME=self.home, **extra)
        self.players = players
        try:
            self.ctl(1, 'launch', *(['--wait'] if ready else []), '--world', 'Split', '--creative',
                     '--resources', str(RESOURCES), '--width', str(WIDTH), '--height', str(HEIGHT),
                     '--render-distance', '4', '--name', 'P1',
                     # Another scenario's windows closing take the focus: no pause on losing it.
                     '--no-pause', *args)
            if not ready:
                self.wait(lambda: self.alive(1), "P1's game")
                return
            self.wait(lambda: all(self.alive(n) for n in range(1, players + 1)), 'every player in')
            # The automatic GUI scale keeps menus about 400 x 240: room for every row of the page.
            self.ctl(1, 'options-set', 'guiScale', '2')
        except BaseException:
            # A launch that never got ready leaves no game behind.
            self.quit()
            raise

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
        """Open player n's Splitscreen Setup screen (Horizon Settings > Splitscreen...), on or
        off; its widgets."""
        def shown():
            labels = self.labels(n)
            return 'Done' in labels and ('Identify' in labels or 'Splitscreen: Off' in labels)
        if shown():
            return self.ctl(n, 'ui')['widgets']  # still open (Esc taken during a rebuild)
        self.ctl(n, 'options', 'horizon')
        try:
            self.wait(lambda: 'Splitscreen...' in self.labels(n), 'Horizon Settings', seconds=15)
        except RuntimeError:
            ui = self.ctl(n, 'ui')
            raise RuntimeError(f'no Horizon Settings: screen {ui.get("screen")}, '
                               f'buttons {[w["label"] for w in ui["widgets"]]}') from None
        self.ctl(n, 'click-button', 'Splitscreen...')
        self.wait(shown, 'the Setup screen', seconds=5)
        return self.ctl(n, 'ui')['widgets']

    def setup(self, n=1):
        """Open player n's Splitscreen Setup screen, splitscreen on."""
        self.page(n)
        self.wait(lambda: 'Apply' in self.labels(n), 'the Setup screen', seconds=5)

    def press(self, n, label):
        """Click player n's button `label` (the first whose label starts with it)."""
        ids = [w['id'] for w in self.ctl(n, 'ui')['widgets'] if w.get('label', '').startswith(label)]
        if not ids:
            raise RuntimeError(f'no {label!r} among {self.labels(n)}')
        self.ctl(n, 'click', ids[0])
        time.sleep(0.4)

    def play_on(self, player, where, by=1):
        """Move player `player` to `where` ('Window 2', 'New window', 'Fill Monitor 2') on player
        `by`'s Setup screen and Apply; a window not open yet is a new one."""
        self.setup(by)
        self.press(by, f'P{player}')
        if where.startswith('Window '):
            # One window button cycles through the windows: on until it names this one (or a new
            # window when it never does: the window isn't open yet).
            for _ in range(8):
                windows = [l for l in self.labels(by) if l.startswith('Window ')]
                if not windows or where in windows:
                    break
                self.press(by, windows[0])
        self.press(by, where if where in self.labels(by) else 'New window')
        self.press(by, 'Apply')
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
            # Not answering (still starting, or stuck): stop it by its socket (`--control-socket`).
            subprocess.run(['pkill', '-f', str(self.sockets)], capture_output=True)
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
        # A player's quit leaves splitscreen; only P1's ends the game.
        g.ctl(3, 'quit')
        g.wait(lambda: not g.alive(3), 'P3 gone after its quit', seconds=15)
        check("P3's quit: P3 left, P1 plays on", g.ctl(1, 'state').get('player') is not None, True)
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
            g.setup()
            g.press(1, '3 sharing:')
            g.press(1, 'Apply')
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
    print('picker: rename and delete a profile, RB to a new window, then a new profile')

    def seed(home):
        (home / 'splitscreen/profiles/Zed').mkdir(parents=True)

    g = Game(2, pick=True, seed=seed)
    try:
        menu = lambda: g.ctl(2, 'state').get('menu') or {}  # noqa: E731
        g.wait(lambda: menu().get('mode') == 'splitscreen-profiles', 'the picker')
        check('Zed on the list', menu().get('profiles'), ['Zed'])
        # Edit: renamed Zed -> Zoe (its folder moves), then Edit, Delete, and Delete again to
        # confirm.
        g.ctl(2, 'click-button', 'Edit')
        g.ctl(2, 'type', '--replace', 'Zoe')
        g.ctl(2, 'click-button', 'Save')
        g.wait(lambda: (g.home / 'splitscreen/profiles/Zoe').exists(), 'Zed renamed Zoe', seconds=5)
        check('renamed on the list', menu().get('profiles'), ['Zoe'])
        g.ctl(2, 'click-button', 'Edit')
        g.ctl(2, 'click-button', 'Delete')
        g.ctl(2, 'click-button', 'Delete')
        g.wait(lambda: not (g.home / 'splitscreen/profiles/Zoe').exists(), "Zoe's folder gone", seconds=5)
        check('Zoe deleted', menu().get('profiles'), [])
        half = g.size(1)  # P2 picks in Window 1 beside P1
        g.ctl(2, 'pad', 'rb', 'tap')  # Window 1 -> New window
        g.settle([1], [(2 * half[0], half[1])])
        g.ctl(2, 'click-button', 'New Profile...')
        g.ctl(2, 'type', 'Sam')
        g.ctl(2, 'click-button', 'Create')
        g.wait(lambda: g.ctl(2, 'state').get('player') is not None, 'Sam in the world')
        check('P1 has Window 1 to itself', g.settle([1], [(2 * half[0], half[1])]), [(2 * half[0], half[1])])
        check('Sam plays in its own window', g.size(2) != g.size(1), True)
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
        try:
            g.wait(lambda: g.size(2)[0] > windowed[0], "P2's window fullscreen", seconds=10)
        except RuntimeError:
            # macOS ignores a fullscreen request while another window's transition runs: again.
            g.ctl(2, 'key', 'f11', 'tap')
            g.wait(lambda: g.size(2)[0] > windowed[0], "P2's window fullscreen", seconds=20)
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
        g.ctl(2, 'pad', 'lb', 'tap')
        check('LB: back beside P1 in Window 1', g.settle([1, 2], [half, half]), [half, half])
        g.ctl(2, 'pad', 'rb', 'tap')
        g.settle([2], [own])
        g.ctl(2, 'click-button', 'New Profile...')
        g.ctl(2, 'type', 'Ann')
        g.ctl(2, 'click-button', 'Create')
        g.wait(lambda: g.ctl(2, 'state').get('player') is not None, 'Ann in the world')
        check('Ann joins where the bumpers put her', g.settle([1, 2], [whole, own]), [whole, own])
    finally:
        g.quit()
    print('bumpers: LB puts a joining player before P1 in Window 1, and it stays there')
    g = Game(2, pick=True)
    try:
        g.wait(lambda: (g.ctl(2, 'state').get('menu') or {}).get('mode') == 'splitscreen-profiles',
               'the picker')
        half = g.size(1)
        g.ctl(2, 'pad', 'lb', 'tap')  # from "Window 1 (2 of 2)" to "(1 of 2)"
        time.sleep(0.5)
        g.ctl(2, 'click-button', 'New Profile...')
        g.ctl(2, 'type', 'Bea')
        g.ctl(2, 'click-button', 'Create')
        g.wait(lambda: g.ctl(2, 'state').get('player') is not None, 'Bea in the world')
        check('still halves of Window 1', g.settle([1, 2], [half, half]), [half, half])
        # Who is first shows on the Setup screen's map.
        g.setup(1)
        tiles = {w['label']: w['bounds'] for w in g.ctl(1, 'ui')['widgets']}
        check('Bea before P1: left of it', tiles['Bea'][0] < tiles['P1'][0], True)
        g.press(1, 'Done')
    finally:
        g.quit()


def scenario_leave():
    print('leave: from the pause menu, or hold Select then X to switch profile')
    g = Game(3)
    try:
        def hold_select(n):
            g.ctl(n, 'pad', 'select', 'down')
            time.sleep(2.5)
            g.ctl(n, 'pad', 'select', 'up')
            time.sleep(0.3)
        # Its splitscreen menu doesn't leave (A closes it); its pause menu does.
        hold_select(3)
        g.ctl(3, 'pad', 'south', 'tap')
        time.sleep(1)
        check('A in the splitscreen menu: P3 still plays', g.alive(3), True)
        g.ctl(3, 'key', 'esc', 'tap')
        time.sleep(0.6)
        g.press(3, 'Leave')
        g.wait(lambda: not g.alive(3), 'P3 gone', seconds=10)
        w, h = g.size(1)
        check('Leave: P3 left, P1 and P2 share the window', g.settle([1, 2], [(w, h), (w, h)]), [(w, h), (w, h)])
        hold_select(2)
        g.ctl(2, 'pad', 'west', 'tap')
        g.wait(lambda: g.alive(2) and (g.ctl(2, 'state').get('menu') or {}).get('mode') == 'splitscreen-profiles',
               'P2 choosing again', seconds=15)
        check('X: P2 chooses a profile again, in the same window', g.size(2), (w, h))
        # P1 holding Select is asked about its controller; B keeps it, and P1 plays on.
        hold_select(1)
        g.ctl(1, 'pad', 'east', 'tap')
        time.sleep(0.5)
        check('P1 plays on after keeping its controller', g.ctl(1, 'status')['fps'] > 10, True)
    finally:
        g.quit()
    print('leave: in the hold-Select menu, the bumpers move the view; B keeps it there')
    g = Game(2)
    try:
        half = g.size(1)
        g.ctl(2, 'pad', 'select', 'down')
        time.sleep(2.5)
        g.ctl(2, 'pad', 'select', 'up')
        time.sleep(0.3)
        g.ctl(2, 'pad', 'rb', 'tap')  # from "Window 1 (2 of 2)" to a new window
        whole = (2 * half[0], half[1])
        check('RB: P2 to a new window, P1 alone', g.settle([1], [whole]), [whole])
        g.ctl(2, 'pad', 'east', 'tap')  # Close
        time.sleep(1)
        check('B: P2 stays in its new window, still playing', (g.alive(2), g.size(2) != half), (True, True))
    finally:
        g.quit()


def scenario_signedin():
    print('signedin: P1 signed into an account, another player joins under its own profile')
    session = Path(tempfile.mkdtemp(prefix='horizon-session-')) / 'session.json'
    session.write_text(json.dumps({'name': 'Signed', 'uuid': '069a79f444e94726a5befca90e38aaf5',
                                   'access_token': 'not-a-real-token', 'user_type': 'msa'}))
    session.chmod(0o600)
    g = Game(2, args=('--session-file', str(session)))
    try:
        g.wait(lambda: g.ctl(2, 'state').get('player') is not None, 'P2 in the world', seconds=20)
        time.sleep(2)
        check('P2 plays', g.ctl(2, 'state').get('player') is not None, True)
        check('P1 still plays (not replaced by its own name joining)',
              g.ctl(1, 'state').get('player') is not None and g.size(1) == g.size(2), True)
    finally:
        g.quit()


def scenario_monitor():
    print('monitor: a player fills another monitor, another joins it (skipped with one monitor)')
    g = Game(2)
    try:
        half = g.size(1)
        g.setup(1)
        g.press(1, 'P2')
        fill = [l for l in g.labels(1) if l.startswith('Fill Monitor')]
        if not fill:
            print('  skip one monitor only')
            return
        g.press(1, fill[0])
        g.press(1, 'Apply')
        g.close_page(1)
        g.wait(lambda: g.size(2) != half, "P2's window to fill the monitor", seconds=15)
        time.sleep(1)
        size = g.size(2)
        check('P2 fills the monitor: a window of its own', size != g.size(1), True)
        g.play_on(1, 'Window 2')
        half = (size[0] // 2, size[1])
        g.wait(lambda: g.size(1) == half, 'P1 beside P2 on the monitor', seconds=15)
        check('P1 joins it: halves of the monitor', (g.size(1), g.size(2)), (half, half))
    finally:
        g.quit()


def scenario_setup():
    print('setup: the Setup screen moves a player only on Apply; Revert undoes a draft')
    g = Game(2)
    try:
        w, h = g.size(1)
        g.setup(1)
        g.press(1, 'P2')
        g.press(1, 'New window')
        time.sleep(1)
        check('a draft changes nothing yet', (g.size(1), g.size(2)), ((w, h), (w, h)))
        g.press(1, 'Revert')
        g.press(1, 'P2')
        g.press(1, 'New window')
        g.press(1, 'Apply')
        g.wait(lambda: g.size(1) == (2 * w, h), 'P1 alone in Window 1 after Apply', seconds=15)
        check('Apply: P1 has Window 1 to itself', g.size(1), (2 * w, h))
        check('Apply: P2 in a window of its own', g.size(2) != (w, h), True)
        # Back beside P1 from the screen: its window closes.
        g.press(1, 'P2')
        g.press(1, 'Window 1')
        g.press(1, 'Apply')
        g.wait(lambda: g.size(2) == (w, h), 'P2 back beside P1', seconds=15)
        check('P2 back beside P1', (g.size(1), g.size(2)), ((w, h), (w, h)))
        g.press(1, 'Done')
        check('Done closes the screen', 'Apply' in g.labels(1), False)
        g.close_page(1)
    finally:
        g.quit()
    print('setup: from the controller (hold Select, Y); another player moves only itself')
    g = Game(3)
    try:
        g.ctl(2, 'pad', 'select', 'down')
        time.sleep(2.5)
        g.ctl(2, 'pad', 'select', 'up')
        time.sleep(0.3)
        g.ctl(2, 'pad', 'north', 'tap')
        g.wait(lambda: 'Apply' in g.labels(2), "P2's Setup screen", seconds=10)
        tiles = {w['label']: w for w in g.ctl(2, 'ui')['widgets'] if w.get('label', '').startswith('P')}
        check("P2's screen: P2 can be moved, P3 and P1 not",
              [tiles[k].get('enabled', True) for k in sorted(tiles)], [False, True, False])
        check('no switch for P2', 'Splitscreen: On' in g.labels(2), False)
        g.press(2, 'P2')
        g.press(2, 'New window')
        # Meanwhile P1 sends P3 to a window of its own: P2's draft, older, must not undo it.
        g.setup(1)
        g.press(1, 'P3')
        g.press(1, 'New window')
        g.press(1, 'Apply')
        g.wait(lambda: g.size(3) == (1708, 960), 'P3 in a new window', seconds=15)
        g.press(2, 'Apply')
        g.wait(lambda: g.size(2) == (1708, 960), 'P2 in a new window', seconds=15)
        check("P2's Apply moved only P2: P3 still in its window", g.size(3), (1708, 960))
        g.press(1, 'Done')
        check('P2 moved itself to a window of its own', g.size(2), (1708, 960))
        check('P1 alone in Window 1', g.size(1), (2000, 1200))
        g.press(2, 'Done')
        check('Done: P2 plays again', 'Apply' in g.labels(2), False)
    finally:
        g.quit()
    print('setup: with the controller, A picks a player up, the stick moves it, A drops it')
    g = Game(2)
    try:
        w, h = g.size(1)
        g.setup(1)

        def tap(button):
            g.ctl(1, 'pad', button, 'tap')
            time.sleep(0.35)

        def apply_on():
            return [x.get('enabled') for x in g.ctl(1, 'ui')['widgets'] if x.get('label') == 'Apply']
        # The map's tiles come with the game's first sync of the screen.
        g.wait(lambda: any(l == 'P2' for l in g.labels(1)), "P2's tile", seconds=5)
        tap('right')  # the focus starts on P1's tile
        tap('right')  # P2's
        tap('south')  # picked up
        # The stick to the right: shown in a new window (the D-pad and LB/RB cycle the same way).
        g.ctl(1, 'pad', 'stick', 'left', '1', '0')
        time.sleep(0.25)  # a poll or two, under the menu's repeat (0.35 s)
        g.ctl(1, 'pad', 'stick', 'left', '0', '0')
        time.sleep(0.35)
        check('held: nothing in the draft yet', apply_on(), [False])
        tap('south')  # dropped there
        check('dropped: in the draft', apply_on(), [True])
        g.press(1, 'Apply')
        g.wait(lambda: g.size(1) == (2 * w, h), 'P1 alone in Window 1', seconds=15)
        check('P2 in a new window, P1 alone', (g.size(1), g.size(2) != (w, h)), ((2 * w, h), True))
    finally:
        g.quit()


@contextlib.contextmanager
def dedicated(**extra):
    """A Horizon dedicated server, offline mode, started and ready: (server, port)."""
    from test_dedicated_server_parity import Server
    port = lab.free_port()
    folder = Path(tempfile.mkdtemp(prefix='hz-split-server-'))
    directory = folder / 'server'
    directory.mkdir()
    properties = {'online-mode': 'false', 'white-list': 'false', 'server-port': str(port), 'level-type': 'minecraft\\:flat',
                  'generate-structures': 'false', 'view-distance': '3', 'simulation-distance': '3',
                  'pause-when-empty-seconds': '-1', 'spawn-protection': '0', **extra}
    (directory / 'server.properties').write_text(''.join(f'{k}={v}\n' for k, v in properties.items()),
                                                 encoding='iso-8859-1')
    (directory / 'eula.txt').write_text('eula=true\n')
    server = Server([lab.cargo_target() / 'fast/horizon-client', '--resources', RESOURCES, '--dedicated',
                     '--nogui'], directory, 'server')
    try:
        server.wait_for(r'Done \(')
        yield server, port
    finally:
        server.stop()
        shutil.rmtree(folder, ignore_errors=True)


def scenario_packs():
    print("packs: the server's resource pack: P1 says Yes, the others follow without being asked")
    from server_pack_e2e import Files, build_pack
    folder = Path(tempfile.mkdtemp(prefix='hz-split-pack-'))
    sha1 = build_pack(folder / 'pack.zip')
    files = Files({'/pack.zip': (folder / 'pack.zip').read_bytes()})
    try:
        with dedicated(**{'resource-pack': files.url('/pack.zip'), 'resource-pack-sha1': sha1,
                          'require-resource-pack': 'false'}) as (server, port):
            g = Game(2, args=('--server', f'127.0.0.1:{port}'), ready=False)
            try:
                g.wait(lambda: 'Yes' in g.labels(1), "P1 asked about the server's pack", seconds=60)
                g.ctl(1, 'click-button', 'Yes')
                server.wait_for(r'P2 joined the game', timeout=90)
                check("P2 joins on P1's Yes, never asked itself", True, True)
                # P2 finds P1's copy in their shared downloads folder (same SHA-1).
                downloads = [p for p, _ in files.requests if p == '/pack.zip']
                check('the pack downloaded once, for both', len(downloads), 1)
            finally:
                g.quit()
    finally:
        files.shutdown()
        shutil.rmtree(folder, ignore_errors=True)


def scenario_server():
    print('server: P1 on a dedicated server; the others join it, a kicked one drops out, all leave with P1')
    with dedicated() as (server, port):
        g = Game(3, args=('--server', f'127.0.0.1:{port}'))
        try:
            server.wait_for(r'P2 joined the game', timeout=60)
            server.wait_for(r'P3 joined the game', timeout=60)
            check('P2 and P3 join the server P1 is on', True, True)
            server.console('kick P3 Testing')
            g.wait(lambda: not g.alive(3), 'P3 gone after its kick', seconds=20)
            check('kicked: P3 drops out, P1 plays on', g.ctl(1, 'state').get('player') is not None, True)
            g.ctl(1, 'disconnect')
            server.wait_for(r'P2 left the game', timeout=30)
            check('P1 leaves: P2 leaves with it', True, True)
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



def scenario_pointer():
    print('pointer: only the player with the keyboard and mouse grabs the pointer')
    g = Game(2)
    try:
        time.sleep(3)
        grabbed = lambda n: g.ctl(n, 'state').get('pointer_grabbed')  # noqa: E731
        g.ctl(1, 'mouse', 'left', 'click')
        time.sleep(0.5)
        check('P1 plays with the mouse, P2 never grabs it', (grabbed(1), grabbed(2)), (True, False))
        # P2's menus (its controller) leave P1's pointer alone.
        g.ctl(2, 'key', 'e', 'tap')
        time.sleep(0.6)
        g.ctl(2, 'key', 'esc', 'tap')
        time.sleep(0.6)
        check("after P2's inventory: still P1's", (grabbed(1), grabbed(2)), (True, False))
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
    'leave': scenario_leave,
    'signedin': scenario_signedin,
    'monitor': scenario_monitor,
    'setup': scenario_setup,
    'server': scenario_server,
    'packs': scenario_packs,
    'pointer': scenario_pointer,
}


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--only', nargs='+', choices=SCENARIOS, metavar='SCENARIO',
                    help=f'run only these: {", ".join(SCENARIOS)}')
    args = ap.parse_args()
    if not CTL.exists():
        sys.exit(f'{CTL} is missing: cargo build --profile fast -p horizon-client')
    if not (RESOURCES / 'assets').exists():
        sys.exit(f'no prepared resources in {RESOURCES}')
    for name, run in SCENARIOS.items():
        if args.only is None or name in args.only:
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
