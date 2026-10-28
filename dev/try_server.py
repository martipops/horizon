#!/usr/bin/env python3
"""Try splitscreen on a server by hand (dev only): python3 dev/try_server.py

Starts a Horizon dedicated server on localhost (offline mode, a flat world), launches the game
with P1 on it, adds a second player who stops at the profile picker, then waits: play with the
windows, and drive P2 with its virtual controller from another shell (the commands are printed).
Ctrl-C, or closing the game, ends it all. Uses throwaway folders, never your real game's.

    --players N    players in all (default 2; the others stop at the picker too)
    --server HOST  join this server (yours, already running) instead of starting one
    --launcher     the picker lists your launcher's offline names and other accounts
                   (as dev/launch.py's "Splitscreen profiles: launcher")
"""
import argparse
import contextlib
import os
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tools'))
sys.path.insert(0, str(ROOT / 'dev'))
import splitscreen_e2e as e  # noqa: E402


def wait_for(address, seconds=300):
    """Until the server at `address` takes connections (it may still be starting)."""
    import socket
    host, _, port = address.rpartition(':')
    end = time.time() + seconds
    while time.time() < end:
        try:
            socket.create_connection((host or address, int(port or 25565)), timeout=2).close()
            return
        except OSError:
            print('  waiting for the server...', flush=True)
            time.sleep(5)
    sys.exit(f'No server at {address}')


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--players', type=int, default=2)
    ap.add_argument('--launcher', action='store_true')
    ap.add_argument('--server')
    args = ap.parse_args()
    e.WIDTH, e.HEIGHT = 1280, 720
    sessions = []
    started = contextlib.nullcontext((None, None)) if args.server else e.dedicated()
    with started as (_, port):
        address = args.server or f'127.0.0.1:{port}'
        print(f'Server on {address}', flush=True)
        if args.server:
            wait_for(address)
        if args.launcher:
            import launch
            env, sessions = launch.profiles_env(Path(e.tempfile.mkdtemp(prefix='hz-try-')), None)
            os.environ.update(env)
        g = e.Game(args.players, pick=True, args=('--server', address))
        try:
            g.wait(lambda: (g.ctl(2, 'state').get('menu') or {}).get('mode') == 'splitscreen-profiles',
                   "P2's profile picker", seconds=60)
            ctl = f'{e.CTL} --socket {g.sockets}'
            print('\nP1 is on the server; P2 waits at the profile picker. Drive P2 from another shell:')
            print(f'  {ctl}/c.sock2 pad rb tap            # where its view goes (lb: back)')
            print(f'  {ctl}/c.sock2 click-button "New Profile..."')
            print(f'  {ctl}/c.sock2 type Sam ; {ctl}/c.sock2 click-button Create')
            print(f'  {ctl}/c.sock2 state                  # what it shows')
            print('Ctrl-C (or closing the game) ends it.\n', flush=True)
            while g.alive(1):
                time.sleep(2)
        except KeyboardInterrupt:
            pass
        finally:
            g.quit()
            for f in sessions:
                f.unlink(missing_ok=True)


if __name__ == '__main__':
    main()
