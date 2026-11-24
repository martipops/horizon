"""Unit tests for tools/splitscreen_e2e.py's runner: no game is started, `Game.ctl` is faked."""
import os
import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from tools import splitscreen_e2e as e


class Runner(unittest.TestCase):
    def setUp(self):
        self.resources = Path(tempfile.mkdtemp(prefix='hz-split-e2e-test-'))
        (self.resources / 'assets').mkdir()
        self.addCleanup(shutil.rmtree, self.resources, ignore_errors=True)
        self.lines = []
        patches = [
            mock.patch.object(e, 'CTL', Path(__file__)),
            mock.patch.object(e, 'RESOURCES', self.resources),
            mock.patch.object(e, 'failures', []),
            mock.patch.object(e, 'OFFSCREEN', False),
            mock.patch.object(e, 'VERBOSE', False),
            mock.patch('builtins.print', side_effect=lambda *words, **_: self.lines.append(' '.join(map(str, words)))),
        ]
        for patch in patches:
            patch.start()
            self.addCleanup(patch.stop)

    def run_main(self, scenarios, *argv):
        with mock.patch.object(e, 'SCENARIOS', scenarios), mock.patch.object(sys, 'argv', ['splitscreen_e2e.py', *argv]):
            e.main()

    def test_a_scenarios_own_error_fails_it_and_the_next_ones_still_run(self):
        ran = []
        scenarios = {
            'stale': lambda: [][0],
            'after': lambda: ran.append('after'),
        }
        with self.assertRaises(SystemExit) as stopped:
            self.run_main(scenarios)
        self.assertEqual(ran, ['after'])
        self.assertEqual(str(stopped.exception.code), '1 check(s) failed: stale')
        self.assertIn('  FAIL stale: IndexError: list index out of range', self.lines)

    def test_offscreen_skips_what_needs_a_real_window_without_failing_it(self):
        ran = []
        scenarios = {
            'fullscreen': lambda: ran.append('fullscreen'),
            'join': lambda: ran.append('join'),
        }
        self.run_main(scenarios, '--offscreen')
        self.assertEqual(ran, ['join'])
        # A run that left something out never says "every check passed".
        self.assertIn('fullscreen', self.lines[-1])
        self.assertNotEqual(self.lines[-1], 'splitscreen: every check passed')
        # Without the option the same scenario runs, and the run is whole.
        self.run_main(scenarios)
        self.assertEqual(ran, ['join', 'fullscreen', 'join'])
        self.assertEqual(self.lines[-1], 'splitscreen: every check passed')

    def test_a_selection_that_only_skipped_is_not_a_pass(self):
        ran = []
        with self.assertRaises(SystemExit) as stopped:
            self.run_main({'fullscreen': lambda: ran.append('fullscreen')}, '--offscreen', '--only', 'fullscreen')
        self.assertEqual(ran, [])
        self.assertNotIn(stopped.exception.code, (0, None))

    def test_a_launcher_scenario_that_fails_leaves_no_accounts_for_the_next_one(self):
        folder = Path(tempfile.mkdtemp(prefix='hz-split-e2e-test-'))
        self.addCleanup(shutil.rmtree, folder, ignore_errors=True)
        given = {}

        def game(*args, **env):
            given.update(env)
            raise KeyError('pid')

        with mock.patch.object(e, 'Game', game), mock.patch.object(e.tempfile, 'mkdtemp', return_value=str(folder)):
            with self.assertRaises(KeyError):
                e.scenario_launcher()
        # The fake accounts went to that game, and the next scenario's game inherits none.
        self.assertIn('Ann', given['HORIZON_ACCOUNT_SESSIONS'])
        self.assertNotIn('HORIZON_ACCOUNT_SESSIONS', os.environ)

    def launch_words(self, **kwargs):
        """The words of the `launch` a `Game` sends, with horizon-ctl faked."""
        sent = []

        def ctl(game, n, *words):
            sent.append(words)
            return {'pid': 1}

        with mock.patch.object(e.Game, 'ctl', ctl):
            game = e.Game(1, ready=False, **kwargs)
        self.addCleanup(shutil.rmtree, game.home, ignore_errors=True)
        self.addCleanup(shutil.rmtree, game.sockets, ignore_errors=True)
        return next(words for words in sent if words[0] == 'launch')

    def test_offscreen_reaches_every_launch_once(self):
        self.assertNotIn('--offscreen', self.launch_words())
        with mock.patch.object(e, 'OFFSCREEN', True):
            self.assertEqual(self.launch_words().count('--offscreen'), 1)
            # A scenario that already asks for it gets it once.
            self.assertEqual(self.launch_words(args=('--offscreen',)).count('--offscreen'), 1)


if __name__ == '__main__':
    unittest.main()
