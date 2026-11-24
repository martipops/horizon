#!/usr/bin/env python3
"""Splitscreen's controllers end to end: which player each drives, with scripted Linux controllers.

    python3 tools/splitscreen_pads_e2e.py                    # every case
    python3 tools/splitscreen_pads_e2e.py --only accept_twice

The controllers are evdev ones made through /dev/uinput (`tools/uinput_pad.py`), so gilrs reads
them as plugged in and presses, unplugs and presses within one poll reach the splitscreen router,
which `horizon-ctl pad` doesn't. Needs write access to /dev/uinput (`sudo setfacl -m u:$USER:rw
/dev/uinput`, until the next reboot). Every program that reads controllers sees them: close other
games, and unplug real controllers.

The games run `--offscreen`. Exit status 0 when every check passed.
"""

import argparse
import sys
import time
from pathlib import Path

try:
    from . import lab, splitscreen_e2e as e2e, uinput_pad
except ImportError:
    import lab
    import splitscreen_e2e as e2e
    import uinput_pad

check = e2e.check
# `owners::OFFER`, and a moment for the frame after it.
OFFER = 11
# Between two controllers plugged in: gilrs reads one connection per wakeup.
PLUG = 1


class Session:
    """P1's game in a world, with splitscreen on or off, and the controllers plugged in."""

    def __init__(self, pads, splitscreen=True, same_model=False):
        self.pads = []
        self.game = e2e.Game(1, split=False, saved_split=splitscreen, args=('--offscreen',))
        try:
            time.sleep(2)
            for k in range(pads):
                self.pads.append(uinput_pad.Pad(f'Test pad {"ABC"[k]}', product=1 if same_model else k + 1))
                time.sleep(PLUG)
            time.sleep(1)
        except BaseException:
            self.close()
            raise

    def close(self):
        for pad in self.pads:
            pad.unplug()
        self.game.quit()

    def status(self, n=1):
        return self.game.ctl(n, 'pad', 'status')

    def pad_of(self, n):
        """The controllers player n plays with, by letter."""
        if not self.game.alive(n):
            return None
        return ''.join(p['name'][-1] for p in self.status(n)['controllers'] if p['active'])

    def prompt(self, n=1):
        return self.status(n)['prompt']

    def choosing(self):
        """The players on the profile page."""
        return [n for n in range(2, 5) if self.game.alive(n)
                and (self.game.ctl(n, 'state').get('menu') or {}).get('mode') == 'splitscreen-profiles']

    def becomes(self, name, got, want, seconds=15):
        try:
            self.game.wait(lambda: got() == want, name, seconds=seconds)
        except RuntimeError:
            pass
        check(name, got(), want)

    def give_p1(self, pad):
        pad.tap('rb')
        self.game.wait(lambda: self.prompt(), 'the join question', 5)
        pad.tap('north')
        letter = pad.name[-1]
        self.game.wait(lambda: self.pad_of(1) == letter, f'P1 plays with {letter}', 5)

    def join(self, pad, name):
        n = next(n for n in range(2, 5) if not self.game.alive(n))
        pad.tap('rb')
        self.game.wait(lambda: self.prompt(), 'the join question', 5)
        pad.tap('south')
        self.game.wait(lambda: n in self.choosing(), f'P{n} choosing', 40)
        self.game.press(n, 'New Profile...')
        self.game.ctl(n, 'type', name)
        self.game.press(n, 'Create')
        self.game.wait(lambda: self.game.ctl(n, 'state').get('player') is not None, f'{name} playing', 40)
        time.sleep(1)

    def switch(self, was):
        self.game.page()
        self.game.press(1, f'Splitscreen: {was}')
        time.sleep(1)
        self.game.close_page()
        time.sleep(0.5)

    def give_up(self):
        self.game.page()
        self.game.press(1, 'Give Up Pad')
        time.sleep(0.5)
        self.game.close_page()


def keep(s, a):
    """P1 keeps its controller as splitscreen turns on in a world, and off, and on"""
    a.tap('west')
    check('P1 plays with A', s.pad_of(1), 'A')
    for was in ('Off', 'On', 'Off'):
        s.switch(was)
        check(f'the switch from {was}: still', s.pad_of(1), 'A')
        a.tap('rb')
        time.sleep(0.5)
        check('its next press asks nothing', s.prompt(), None)
    a.tap('south')
    time.sleep(3)
    check('and A joins nobody', s.game.alive(2), False)


def keyboard(s, a):
    """a controller P1 never used is offered to join, not taken by P1"""
    s.switch('Off')
    a.tap('rb')
    s.becomes('asked', lambda: s.prompt(), 'Test pad A wants to join', 3)
    check('not P1\'s', s.pad_of(1), '')


def accept_twice(s, a):
    """accept, another button and accept in one poll: one player"""
    a.tap('rb')
    time.sleep(0.5)
    a.burst('south', 'rb', 'south')
    s.becomes('P2 chooses', s.choosing, [2], 40)
    time.sleep(3)
    check('alone', s.choosing(), [2])
    check('with A', s.pad_of(2), 'A')


def accept_and_claim(s, a):
    """accept, another button and Y in one poll: P2's, not P1's too"""
    a.tap('rb')
    time.sleep(0.5)
    a.burst('south', 'rb', 'north')
    s.becomes('P2 chooses', s.choosing, [2], 40)
    check('P2 has A', s.pad_of(2), 'A')
    check('P1 has none', s.pad_of(1), '')


def lost_seat(s, a, b):
    """the controller P1 gave up takes a lost player's seat: its offer ends, and it stays there"""
    s.join(b, 'Sam')
    s.give_p1(a)
    s.give_up()
    s.becomes('A is offered', lambda: s.prompt(), 'Test pad A wants to join', 3)
    b.unplug()
    time.sleep(1)
    a.tap('south')
    s.becomes('P2 has A', lambda: s.pad_of(2), 'A', 5)
    check('nobody is asked about it any more', s.prompt(), None)
    time.sleep(OFFER)
    check('later, P2 has it', s.pad_of(2), 'A')
    check('and P1 none', s.pad_of(1), '')


def hand_off(s, a):
    """a hand-off goes on when its controller comes back, and ends with it P1's again"""
    s.give_p1(a)
    s.give_up()
    s.becomes('A is offered', lambda: s.prompt(), 'Test pad A wants to join', 3)
    a.unplug()
    s.becomes('gone: nobody is asked', lambda: s.prompt(), None, 5)
    a.plug()
    s.becomes('back: offered again', lambda: s.prompt(), 'Test pad A wants to join', 5)
    time.sleep(OFFER)
    check('nobody joined: P1\'s again', (s.pad_of(1), s.prompt()), ('A', None))


def hand_off_late(s, a):
    """a hand-off that ran out while its controller was away gives it back to P1"""
    s.give_p1(a)
    s.give_up()
    a.unplug()
    time.sleep(OFFER)
    a.plug()
    s.becomes('back: P1\'s', lambda: s.pad_of(1), 'A', 5)
    check('nobody is asked', s.prompt(), None)


def hand_off_choosing(s, a):
    """a hand-off lost on the profile page gives P1 its controller back"""
    s.give_p1(a)
    s.give_up()
    a.tap('south')
    s.becomes('P2 chooses', s.choosing, [2], 40)
    a.unplug()
    s.becomes('P2 leaves', lambda: s.game.alive(2), False)
    a.plug()
    s.becomes('back: P1\'s', lambda: s.pad_of(1), 'A', 5)


def same_model(s, a, b):
    """two of one model away: neither is guessed back, A on one takes the player waiting longest"""
    s.join(a, 'Sam')
    s.join(b, 'Ann')
    a.unplug()
    time.sleep(1)
    b.unplug()
    time.sleep(1)
    check('both wait', (s.pad_of(2), s.pad_of(3)), ('', ''))
    b.plug()
    time.sleep(2)
    check('one is back: whose?', (s.pad_of(2), s.pad_of(3)), ('', ''))
    b.tap('south')
    s.becomes('A on it: P2\'s', lambda: s.pad_of(2), 'B', 5)
    a.plug()
    s.becomes('the other: P3\'s', lambda: s.pad_of(3), 'A', 5)


def controllers_off(s, a):
    """P1 with controllers off: a press on its own offers it to join, and No keeps it P1's"""
    s.give_p1(a)
    s.game.ctl(1, 'options', 'controller')
    s.game.press(1, 'Controller: On')
    s.game.close_page()
    a.tap('rb')
    s.becomes('asked', lambda: s.prompt(), 'Test pad A wants to join', 3)
    a.tap('east')
    s.becomes('No: nobody is asked', lambda: s.prompt(), None, 3)
    check('P1\'s', s.pad_of(1), 'A')


CASES = {
    'keep': (keep, 1, {'splitscreen': False}),
    'keyboard': (keyboard, 1, {'splitscreen': False}),
    'accept_twice': (accept_twice, 1, {}),
    'accept_and_claim': (accept_and_claim, 1, {}),
    'lost_seat': (lost_seat, 2, {}),
    'hand_off': (hand_off, 1, {}),
    'hand_off_late': (hand_off_late, 1, {}),
    'hand_off_choosing': (hand_off_choosing, 1, {}),
    'same_model': (same_model, 2, {'same_model': True}),
    'controllers_off': (controllers_off, 1, {}),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('--only', nargs='+', choices=CASES, help='the cases to run')
    parser.add_argument('--ctl', type=Path, help='horizon-ctl, beside the game it starts')
    parser.add_argument('--resources', type=Path, help='the prepared resources')
    options = parser.parse_args()
    if not uinput_pad.available():
        sys.exit(f'no write access to {uinput_pad.UINPUT} (Linux only: see --help and the file)')
    e2e.CTL = options.ctl.resolve() if options.ctl else e2e.CTL
    e2e.RESOURCES = options.resources.resolve() if options.resources else lab.RESOURCES
    for name in options.only or CASES:
        case, pads, setup = CASES[name]
        print(f'{name}: {case.__doc__}', flush=True)
        try:
            session = Session(pads, **setup)
            try:
                case(session, *session.pads)
            finally:
                session.close()
        except RuntimeError as e:
            check(name, str(e), 'the case to finish')
    print(f'{len(e2e.failures)} failed' if e2e.failures else 'all passed')
    return 1 if e2e.failures else 0


if __name__ == '__main__':
    sys.exit(main())
