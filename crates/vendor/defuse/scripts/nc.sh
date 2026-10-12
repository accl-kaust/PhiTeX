#!/bin/sh
# Runs cargo at low priority inside bubblewrap, in the memory-capped slice.
# Usage: scripts/nc.sh [--net] <cargo args...>   (--net only if crates must be fetched)
NET=
if [ "$1" = "--net" ]; then NET=--net; shift; fi
exec "$(dirname "$0")/run.sh" $NET cargo +nightly "$@"
