rm -rf target/cache
set -e
b=$(cargo test -p phitex-tex --profile profiling --test latex --no-run 2>&1 | grep -o 'target/profiling/[^)]*latex-[0-9a-f]*' | tail -1)
echo "bin $b"
PHITEX_HEARTBEAT=1 PHITEX_HEARTBEAT_EXIT=2800000 PHITEX_THREADS=1 valgrind --tool=callgrind --dump-instr=yes --callgrind-out-file=cg.out $b latex_hello --exact --include-ignored --test-threads=1 2>&1 | grep -E "^hb" | tail -1
callgrind_annotate --threshold=100 cg.out > cg.excl.txt
callgrind_annotate --inclusive=yes --threshold=100 cg.out > cg.incl.txt
head -45 cg.excl.txt
mkdir -p bench/results
cp cg.out cg.excl.txt cg.incl.txt bench/results/
