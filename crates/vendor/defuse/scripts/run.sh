#!/bin/sh
# Runs a command at low priority inside bubblewrap, in the memory-capped
# `defuse.slice` (systemctl --user set-property defuse.slice MemoryMax=32G
# MemorySwapMax=0): a runaway run is killed instead of taking the machine.
# Usage: scripts/run.sh [--net] <cmd...>
NET=
if [ "$1" = "--net" ]; then NET=--net; shift; fi
exec systemd-run --user --slice=defuse.slice --scope -q nice -n 19 ionice -c3 /home/lambda/.local/bin/bw $NET "$@"
