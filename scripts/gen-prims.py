#!/usr/bin/env python3
"""Generate crates/partex-core/src/prims.rs: every "Put each of TeX's
primitives into the hash table" chunk of the merged WEB source, concatenated
in source order (the order fixes hash positions and string numbers, so it
must not change): `init_prim_tex` from tex.web, `init_prim_pdftex` from
pdftex.web, and `generate_etex_prims` from pdftex.web's "Generate all e-TeX
primitives" chunks.

Run after scripts/merge-web.sh:  python3 scripts/gen-prims.py
"""
import re, pathlib

root = pathlib.Path(__file__).resolve().parent.parent
consts = set(re.findall(r"^pub const ([A-Z_0-9]+):",
                        (root / "crates/partex-core/src/web.rs").read_text(), re.M))

def collect(file, pattern):
  web = (root / "target/web" / file).read_text(errors="replace").split("\n")
  chunks, cur, sec = [], None, 0
  for line in web:
    if re.match(r"^@( |\*|$|\t)", line):
        sec += 1
        if cur is not None:
            chunks.append(cur)
            cur = None
    if re.search(pattern, line):
        if cur is not None:
            chunks.append(cur)
        cur = (sec, [line.split("@>=", 1)[1]])
        continue
    if cur is not None:
        if line.startswith("@<") and "@>=" in line:
            chunks.append(cur)
            cur = None
            continue
        cur[1].append(line)
  if cur is not None:
    chunks.append(cur)
  return chunks

# Pascal statements (strip comments, index entries and WEB formatting codes).
def statements(text):
    # Index entries (`@:x}{\\.{...} primitive@>`) contain braces: drop them first.
    text = re.sub(r"@[\^:.].*?@>", " ", text)
    while True:  # comments may nest braces: {... \.{\\x} ...}
        stripped = re.sub(r"\{[^{}]*\}", " ", text)
        if stripped == text:
            break
        text = stripped
    text = re.sub(r"@[\^:.].*?@>", " ", text)
    text = re.sub(r"@[/!#|+;]", " ", text)
    text = re.sub(r"\s+", " ", text)
    toks = re.findall(r'"(?:[^"]|"")*"|:=|[A-Za-z_][A-Za-z_0-9]*|\d+|.', text)
    return toks

def expr(e):
    e = e.strip()
    e = re.sub(r'"(.)"', lambda m: str(ord(m.group(1))), e)
    def ident(m):
        w = m.group(0)
        if w.upper() in consts:
            return w.upper()
        if w == "null_list":
            return "crate::tok::NULL_LIST"
        if w == "mem_bot":
            return "self.mem_bot"
        raise SystemExit(f"unknown identifier {w} in {e}")
    return re.sub(r"\b[A-Za-z_][A-Za-z_0-9]*\b", ident, e)

def rust_str(s):
    inner = s[1:-1].replace('""', '"')
    return 'b"' + inner.replace("\\", "\\\\").replace('"', '\\"') + '"'

def gen(chunks, cite):
  out = []
  for sec, lines in chunks:
    toks = statements(" ".join(lines))
    out.append(f"        // {cite}§{sec}")
    i, skip_next, depth = 0, False, 0
    def stmt_end(j):
        while j < len(toks) and toks[j] not in (";", "end"):
            j += 1
        return j
    while i < len(toks):
        t = toks[i]
        if t in (" ", ";"):
            i += 1
            continue
        if t == "if":
            j = i + 1
            cond = []
            while toks[j] != "then":
                if toks[j] != " ":
                    cond.append(toks[j])
                j += 1
            cond = "".join(cond)
            i = j + 1
            if cond == "false":
                skip_next = True
                continue
            flag = {"mltex_p": "self.params.mltex", "enctex_p": "self.params.enctex"}[cond]
            out.append(f"        if {flag} {{")
            depth += 1
            continue
        if t == "begin":
            i += 1
            continue
        if t == "end":
            if depth:
                out.append("        }")
                depth -= 1
            i += 1
            continue
        if t == "primitive":
            j = i + 2
            name = toks[j]
            j += 1
            args, cur_a, par = [], [], 0
            while True:
                j += 1
                x = toks[j]
                if x == "(":
                    par += 1
                if x == ")":
                    if par == 0:
                        break
                    par -= 1
                if x == "," and par == 0:
                    args.append("".join(cur_a))
                    cur_a = []
                    continue
                cur_a.append(x)
            args.append("".join(cur_a))
            i = j + 1
            line = f"self.primitive({rust_str(name)}, {expr(args[0])}, {expr(args[1])})?;"
            if skip_next:
                out.append(f"        // `if false then`: {line}")
                skip_next = False
            else:
                out.append(f"        {line}")
            continue
        # Other assignments, one statement.
        j = stmt_end(i)
        s = "".join(toks[i:j]).strip()
        i = j
        m = re.fullmatch(r'text\(([a-z_]+)\):=("[^"]*")', s)
        if m:
            out.append(f"        let s = self.pool_str({rust_str(m.group(2))});")
            out.append(f"        self.set_text({expr(m.group(1))}, s);")
            continue
        m = re.fullmatch(r"eqtb\[([a-z_]+)\]:=eqtb\[([a-z_]+)\]", s)
        if m:
            src = "self.cur_val" if m.group(2) == "cur_val" else expr(m.group(2))
            out.append(f"        let w = self.eqtb({src});")
            out.append(f"        self.set_eqtb({expr(m.group(1))}, w);")
            continue
        m = re.fullmatch(r"(eq_type|equiv|eq_level)\(([a-z_]+)\):=([a-z_]+)", s)
        if m:
            out.append(f"        self.set_{m.group(1)}({expr(m.group(2))}, {expr(m.group(3))});")
            continue
        m = re.fullmatch(r"(mltex_enabled_p|enctex_enabled_p):=true", s)
        if m:
            out.append(f"        self.{m.group(1)} = true;")
            continue
        m = re.fullmatch(r"(par_loc|write_loc):=cur_val", s)
        if m:
            out.append(f"        self.{m.group(1)} = self.cur_val;")
            continue
        m = re.fullmatch(r"par_token:=cs_token_flag\+par_loc", s)
        if m:
            out.append("        self.par_token = CS_TOKEN_FLAG + self.par_loc;")
            continue
        if s:
            raise SystemExit(f"unhandled statement in §{sec}: {s!r}")
  return out


def function(name, doc, out):
    return f'''    /// {doc}
    pub(crate) fn {name}(&mut self) -> Result<(), Jump> {{
        self.no_new_control_sequence = false;
{chr(10).join(out)}
        self.no_new_control_sequence = true;
        Ok(())
    }}
'''


tex = gen(collect("tex-merged.web", r"@<Put each.*@>="), "")
pdftex = gen(collect("pdftex-merged.web", r"@<Put each.*@>="), "pdfTeX ")
etex = gen(collect("pdftex-merged.web", r"@<Generate all \\eTeX.*@>="), "pdfTeX ")
fns = "\n".join([
    function("init_prim_tex", "§1336: call `primitive` for each of TeX's primitives (INITEX).", tex),
    function("init_prim_pdftex", "pdfTeX §1336: pdfTeX's primitives (INITEX), in compatibility mode.", pdftex),
    function("generate_etex_prims", "pdfTeX §1650: \"Generate all e-TeX primitives\" on entering extended mode.", etex),
])
src = f'''//! Put each of TeX's primitives into the hash table (`init_prim`).
//! Generated by `scripts/gen-prims.py` from the merged WEB source; do not
//! edit by hand. The order fixes hash positions and string numbers.

use crate::host::Host;
use crate::tex::{{Jump, Tex}};
use crate::track::Tracker;
use crate::web::*;

impl<H: Host, T: Tracker> Tex<H, T> {{
{fns}}}
'''
(root / "crates/partex-core/src/prims.rs").write_text(src)
for n, out in [("tex", tex), ("pdftex", pdftex), ("etex", etex)]:
    print(n, sum(1 for l in out if "self.primitive(" in l and "//" not in l), "primitive calls")
