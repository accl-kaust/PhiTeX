#!/usr/bin/env python3
"""Address lists for the pure SSA tracer's projections (DESIGN 3.17,
"Measured"): from `PARTEX_PURE_NAMES`'s table (key, name), the eqtb
addresses of one cause, one key a line, for `PARTEX_PURE_FIELDS`:

    scripts/pure-fields.py NAMES.tsv CAUSE > LIST

CAUSE is one of:
- alloc: the allocators' ordinals (LaTeX's \\count10-\\count19 and
  \\allocationnumber, expl3's and LaTeX's allocation counters), which an
  allocated register's uses do not read: they read its identity;
- hooks: macros built by appends (\\g@addto@macro, \\AtBeginDocument,
  lthooks' code, expl3's seq, clist and prop appends, the file list),
  whose appends do not read what they append to;
- catcodes: the category code table's entries;
- all: the three together.

The projection run reads none of these as operands: the critical path it
reports is the path with them modelled as the design models them.
"""
import re
import sys

CAUSES = {
    "alloc": re.compile(
        r"^count 1[0-9]\b|\\allocationnumber|\\(e@)?alloc|\\g__\w*alloc|\\c@\w*alloc"
        r"|insc@unt|\\count@|\\g__kernel_\w*_int\b"),
    "hooks": re.compile(
        r"hook|\\@begindocumenthook|\\@enddocumenthook|\\@filelist|\\@\w*list\b"
        r"|_seq\b|_clist\b|_prop\b|\\@preamblecmds|\\@unusedoptionlist|\\@declaredoptions"),
    "catcodes": re.compile(r"^catcode"),
}


def main():
    names, cause = sys.argv[1], sys.argv[2]
    pats = list(CAUSES.values()) if cause == "all" else [CAUSES[cause]]
    for line in open(names, encoding="utf-8", errors="replace"):
        key, _, name = line.rstrip("\n").partition("\t")
        if any(p.search(name) for p in pats):
            print(key)


if __name__ == "__main__":
    main()
