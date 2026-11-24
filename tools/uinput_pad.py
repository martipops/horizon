"""Scripted controllers on Linux: evdev gamepads made through /dev/uinput, which gilrs reads like
plugged-in ones (`horizon-ctl pad` feeds a game's own virtual pad and never reaches gilrs).

Standard library only. The device disappears when it is closed or this process ends. Every program
that reads controllers sees it, so keep runs short and other games closed.
"""

import fcntl
import os
import struct
import time

UINPUT = '/dev/uinput'

_EV_SYN, _EV_KEY, _EV_ABS = 0, 1, 3
_BUS_USB = 3
# linux/uinput.h: _IO('U', n) and _IOW('U', n, size).
_DEV_CREATE, _DEV_DESTROY = 0x5501, 0x5502


def _iow(number, size):
    return 1 << 30 | size << 16 | ord('U') << 8 | number


# struct uinput_setup { struct input_id id; char name[80]; __u32 ff_effects_max; }
_SETUP = '4H80sI'
# struct uinput_abs_setup { __u16 code; struct input_absinfo { 6 * __s32 }; }
_ABS_SETUP = 'H2x6i'
_DEV_SETUP = _iow(3, struct.calcsize(_SETUP))
_ABS = _iow(4, struct.calcsize(_ABS_SETUP))
_SET_EVBIT, _SET_KEYBIT, _SET_ABSBIT = _iow(100, 4), _iow(101, 4), _iow(103, 4)

# linux/input-event-codes.h, by place: an Xbox-style pad's X (west) is BTN_NORTH's code and its Y
# (north) BTN_WEST's (`face_buttons::crossed`).
BUTTONS = {
    'south': 0x130, 'east': 0x131, 'west': 0x133, 'north': 0x134, 'lb': 0x136, 'rb': 0x137,
    'lt': 0x138, 'rt': 0x139, 'select': 0x13a, 'start': 0x13b, 'ls': 0x13d, 'rs': 0x13e,
    'up': 0x220, 'down': 0x221, 'left': 0x222, 'right': 0x223,
}
_STICK = 32767
# ABS_X, ABS_Y, ABS_RX, ABS_RY: all 32 bits signed, resting at 0.
AXES = (0, 1, 3, 4)


def available():
    return os.access(UINPUT, os.W_OK)


class Pad:
    """A controller, plugged in until `unplug` (or the end of a `with`)."""

    def __init__(self, name='Horizon test pad', product=1, vendor=0x1209):
        """`product`: two pads with the same one are the same model. The default vendor is
        pid.codes' test range, in no mapping database, so the kernel's own layout is used."""
        self.name, self.product, self.vendor = name, product, vendor
        self.fd = None
        self.plug()

    def plug(self):
        self.fd = os.open(UINPUT, os.O_WRONLY | os.O_NONBLOCK)
        fcntl.ioctl(self.fd, _SET_EVBIT, _EV_KEY)
        fcntl.ioctl(self.fd, _SET_EVBIT, _EV_ABS)
        for code in BUTTONS.values():
            fcntl.ioctl(self.fd, _SET_KEYBIT, code)
        for code in AXES:
            fcntl.ioctl(self.fd, _SET_ABSBIT, code)
            fcntl.ioctl(self.fd, _ABS, struct.pack(_ABS_SETUP, code, 0, -_STICK, _STICK, 0, 0, 0))
        fcntl.ioctl(self.fd, _DEV_SETUP, struct.pack(_SETUP, _BUS_USB, self.vendor, self.product, 1, self.name.encode(), 0))
        fcntl.ioctl(self.fd, _DEV_CREATE)

    def unplug(self):
        if self.fd is not None:
            fcntl.ioctl(self.fd, _DEV_DESTROY)
            os.close(self.fd)
            self.fd = None

    def _emit(self, events):
        # struct input_event: a timeval the kernel fills, type, code, value.
        os.write(self.fd, b''.join(struct.pack('llHHi', 0, 0, *e) for e in events))

    @staticmethod
    def _key(button, down):
        return [(_EV_KEY, BUTTONS[button], int(down)), (_EV_SYN, 0, 0)]

    def down(self, button):
        self._emit(self._key(button, True))

    def up(self, button):
        self._emit(self._key(button, False))

    def tap(self, button, hold=0.12):
        self.down(button)
        time.sleep(hold)
        self.up(button)
        time.sleep(0.05)

    def burst(self, *buttons):
        """Every button pressed and let go in one write: one poll of the game sees them all."""
        self._emit([e for b in buttons for e in self._key(b, True) + self._key(b, False)])

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.unplug()
