#!/bin/bash
# One part of the proof page's benchmark on GitHub's hosted runners
# (.github/workflows/bench.yml): scripts/ci/bench.py in the CI image's
# -doc variant ($IMAGE-doc), as scripts/accl/bench.sbatch runs it on accl.
# The checkout is the harness (/h, read only), the results go to
# out/bench (/r), the release's binary is out/bin/phitex (/b).
#
#   scripts/ci/bench-gh.sh bin TAG                    the release's Linux x86_64 binary
#   scripts/ci/bench-gh.sh cold CORPUS SHARD OF       its documents SHARD, SHARD+OF, … (by the oracle's wall)
#   scripts/ci/bench-gh.sh edits DOCS                 the edit matrix's DOCS (':'-separated)
set -euo pipefail
cd "$(dirname "$0")/../.."
step=$1
shift
mkdir -p out/bench out/w out/bin
case $step in
  bin)
    tag=$1
    gh release download "$tag" --repo "${GITHUB_REPOSITORY:-accl-kaust/PhiTeX}" -p phitex-x86_64-linux.tar.gz -D out/bin --clobber
    tar -C out/bin -xzf out/bin/phitex-x86_64-linux.tar.gz
    x=$(find out/bin -type f -name phitex | head -1)
    [ "$x" = out/bin/phitex ] || mv "$x" out/bin/phitex
    chmod +x out/bin/phitex
    out/bin/phitex --version | head -1
    exit 0
    ;;
  cold)
    corpus=$1 shard=$2 of=$3
    list=ci/$corpus.json
    # (every OF-th document of the list ordered as bench.py builds it, the
    # longest first: the shards' walls even)
    ids=$(python3 -I - "$list" "$corpus" "$shard" "$of" <<'EOF'
import json, sys
path, corpus, shard, of = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
d = json.load(open(path))
items = d["documents"] if corpus == "research" else d["manuals"]
items = sorted(items, key=lambda c: (-(c.get("oracle_wall_s") or 0), c["id"]))
print(":".join(c["id"] for c in items[shard::of]))
EOF
)
    doc=/usr/share/doc/texlive
    [ "$corpus" = research ] && doc=/h/ci/research
    set -- python3 -I /h/scripts/ci/bench.py cold --list "/h/$list" --corpus "$corpus" --doc "$doc" \
      --bin /b/phitex --out /r --ids "$ids"
    ;;
  edits)
    set -- python3 -I /h/scripts/ci/bench.py edits --bin /b/phitex --docs "$1" --repo /h --out "/r/edits/$(hostname -s)"
    ;;
  *)
    echo "bench-gh.sh: no step $step" >&2
    exit 2
    ;;
esac
# (the machine, as bench.py json reads it: what the page says it ran on)
python3 -I - > "out/bench/machine-$(hostname -s).json" <<'EOF'
import json, os, re, subprocess
cpu = re.search(r"Model name:\s*(.+)", subprocess.run(["lscpu"], capture_output=True, text=True).stdout)
mem = int(open("/proc/meminfo").read().split()[1]) // (1024 * 1024)
print(json.dumps({"cores": os.cpu_count(), "cpu": cpu.group(1).strip() if cpu else None, "mem_gb": mem,
                  "note": "a GitHub-hosted runner (ubuntu-24.04), shared; each document's TeX Live and PhiTeX builds alternate on it, one build at a time"}))
EOF
# (as accl's box: no network, the work and results the only writable places)
docker run --rm --network none \
  -v "$PWD:/h:ro" -v "$PWD/out/bench:/r" -v "$PWD/out/bin:/b:ro" -v "$PWD/out/w:/w" -w /w \
  -e TEXMFVAR=/w/texmf-var -e PHITEX_SANDBOX=1 -e CI_WORK=/w -e HOME=/w/home \
  "$IMAGE-doc" "$@"
