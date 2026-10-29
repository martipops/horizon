#!/bin/sh
# A launcher instance's wrapper (horizon-launcher instance launch NAME --wrapper this): runs this
# branch's game (target/fast, `cargo build --profile fast -p horizon-client`) in place of the
# installed release, with the launcher's arguments and variables: its session, offline names,
# skins and other accounts.
shift
exec "$(cd "$(dirname "$0")/.." && pwd)/target/fast/horizon-client" "$@"
