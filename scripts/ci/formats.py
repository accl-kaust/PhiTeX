#!/usr/bin/env python3
"""partex's own pdflatex and xelatex formats, made by the binary under
test as fmtutil makes TeX Live's (and as `partex build` makes its own:
crates/partex-cli/src/modern.rs, `ensure_format`):

    formats.py PARTEX OUTDIR
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import common  # noqa: E402

FONTS = common.HERE.parent / "xetex" / "fonts.conf"


def main():
    binary, out = Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve()
    out.mkdir(parents=True, exist_ok=True)
    jobs = [
        ("pdflatex", [binary, "--compat=pdftex", "-ini", "-interaction=batchmode", "-jobname=pdflatex",
                      "-translate-file=cp227.tcx", "*pdflatex.ini"], {}),
        ("xelatex", [binary, "--compat=tex", "-engine=xetex", "-ini", "-etex", "-interaction=batchmode",
                     "-jobname=xelatex", "xelatex.ini"],
         {"FONTCONFIG_FILE": str(FONTS), "PARTEX_XETEX": "1"}),
    ]
    status = 0
    for name, cmd, extra in jobs:
        r = common.run(cmd, out, common.env(extra), 900, out=out / f"{name}-term.txt")
        ok = (out / f"{name}.fmt").is_file()
        print(f"format {name}: {'ok' if ok else 'FAILED'} ({r['wall_s']} s, exit {r['exit']})")
        status |= not ok
    # (and `partex build`'s own, which it keeps under PARTEX_FORMATS in a
    # directory named for the binary: made here by a build of a one-line
    # document, so no measured or compared build makes it)
    tiny = out / "warm"
    tiny.mkdir(exist_ok=True)
    (tiny / "tiny.tex").write_text("\\documentclass{article}\\begin{document}x\\end{document}\n")
    r = common.run([binary, "build", "--no-shell-escape", "--color", "never", "tiny.tex"], tiny,
                   common.env({"PARTEX_FORMATS": str(out), "PARTEX_CACHE_DIR": str(tiny / "cache"),
                               "PARTEX_STORE": "0", "HOME": str(tiny)}), 900, out=tiny / "term.txt")
    made = sorted(out.glob("formats-*/pdflatex.fmt"))
    print(f"format of partex build: {'ok' if made else 'FAILED'} ({r['wall_s']} s)")
    status |= not made
    sys.exit(status)


if __name__ == "__main__":
    main()
