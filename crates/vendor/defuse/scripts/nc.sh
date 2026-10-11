#!/bin/sh
# Runs cargo at low CPU and IO priority inside bubblewrap.
# Usage: scripts/nc.sh [--net] <cargo args...>   (--net only if crates must be fetched)
NET=
if [ "$1" = "--net" ]; then NET=--net; shift; fi
exec nice -n 19 ionice -c3 /home/lambda/.local/bin/bw $NET cargo +nightly "$@"
