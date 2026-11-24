"""Unit tests for tools/splitscreen_e2e.py's runner: no game is started, `Game.ctl` is faked."""
import os
import shutil
import subprocess
import sys
import tempfile
import time
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

    def test_a_skip_from_inside_a_scenario_is_not_a_pass_either(self):
        def monitor():
            raise e.Skipped('one monitor only')

        with self.assertRaises(SystemExit) as stopped:
            self.run_main({'monitor': monitor}, '--only', 'monitor')
        self.assertIn('one monitor only', str(stopped.exception.code))
        self.run_main({'monitor': monitor, 'join': lambda: None})
        self.assertIn('monitor', self.lines[-1])
        self.assertNotEqual(self.lines[-1], 'splitscreen: every check passed')

    @unittest.skipIf(os.name == 'nt', 'needs a POSIX shell script and pkill')
    def test_a_game_that_stops_answering_fails_its_scenario_and_the_next_one_runs(self):
        folder = Path(tempfile.mkdtemp(prefix='hz-split-e2e-test-'))
        self.addCleanup(shutil.rmtree, folder, ignore_errors=True)
        # horizon-ctl as a hung game leaves it: the launch answers, then nothing does.
        ctl = folder / 'ctl'
        ctl.write_text('#!/bin/sh\ncase "$*" in *launch*) echo \'{"pid": 1}\';; *) exec sleep 600;; esac\n')
        ctl.chmod(0o755)
        ran, kills = [], []
        real = subprocess.run

        def run(words, **kwargs):
            if words[0] == 'pkill':
                kills.append(words)
                return None
            return real(words, **kwargs)

        scenarios = {'hung': lambda: e.Game(1), 'after': lambda: ran.append('after')}
        start = time.monotonic()
        with mock.patch.object(e, 'CTL', ctl), mock.patch.object(e, 'CALL_SECONDS', 0.2), \
                mock.patch.object(e.subprocess, 'run', run), self.assertRaises(SystemExit) as stopped:
            self.run_main(scenarios)
        self.assertLess(time.monotonic() - start, 10)
        self.assertEqual(ran, ['after'])
        self.assertEqual(str(stopped.exception.code), '1 check(s) failed: hung')
        # The hung game is killed, not left behind.
        self.assertEqual(len(kills), 1)

    def test_a_launcher_scenario_that_fails_leaves_no_accounts_for_the_next_one(self):
        folder = Path(tempfile.mkdtemp(prefix='hz-split-e2e-test-'))
        self.addCleanup(shutil.rmtree, folder, ignore_errors=True)
        given = {}
        # Whatever this process was started with stays as it was.
        kept = mock.patch.dict(os.environ, HORIZON_ACCOUNT_SESSIONS='[]')
        kept.start()
        self.addCleanup(kept.stop)

        def game(*args, **env):
            given.update(env)
            raise KeyError('pid')

        with mock.patch.object(e, 'Game', game), mock.patch.object(e.tempfile, 'mkdtemp', return_value=str(folder)):
            with self.assertRaises(KeyError):
                e.scenario_launcher()
        # The fake accounts went to that game, and the next scenario's game doesn't inherit them.
        self.assertIn('Ann', given['HORIZON_ACCOUNT_SESSIONS'])
        self.assertEqual(os.environ['HORIZON_ACCOUNT_SESSIONS'], '[]')

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
