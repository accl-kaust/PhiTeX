#!/bin/bash
# Room on a GitHub hosted runner for the CI image (docs/ci.md): the
# toolchains it carries that no job here uses.
sudo rm -rf /usr/share/dotnet /usr/local/lib/android /opt/ghc /usr/local/.ghcup /opt/hostedtoolcache/CodeQL /usr/local/share/boost
sudo docker image prune -af > /dev/null
df -h / | tail -1
