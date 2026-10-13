d=$(mktemp -d); cd "$d"; pwd
/usr/bin/time -f "wall %e s" pdftex -ini -etex -interaction=batchmode -output-directory="$d" '*latex.ltx' >/dev/null 2>t.txt; echo "exit $?"; cat t.txt
ls -la "$d"; tail -12 "$d"/latex.log
