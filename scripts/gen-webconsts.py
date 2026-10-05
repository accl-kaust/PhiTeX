#!/usr/bin/env python3
"""Generate crates/partex-engine/src/web.rs: every numeric constant (`@d x=...`
or an arithmetic `@d x==...`) of the merged pdfTeX WEB source (a superset of
tex.web's), in source order, with XeTeX's layout where it differs (DESIGN
4.7). Names that tex.web also defines cite its section (`§N`); pdfTeX's own
cite `pdfTeX §N`, XeTeX's `XeTeX §N`.

Run after scripts/merge-web.sh:  python3 scripts/gen-webconsts.py
"""
import re, pathlib

root = pathlib.Path(__file__).resolve().parent.parent
ID = r"[A-Za-z_][A-Za-z_0-9]*"


def parse(file):
    web = (root / "target/web" / file).read_text(errors="replace").split("\n")
    sec = 0
    defs, order = {}, []
    for line in web:
        if re.match(r"^@( |\*|$|\t)", line):
            sec += 1
        m = re.match(rf"^(?:@ )?@d\s+({ID})\s*(==|=)\s*(.*)$", line)
        if not m:
            continue
        name, _, rest = m.groups()
        rhs = rest.split("{")[0].strip().rstrip(";").strip()
        rhs = re.sub(r"@[;!/|#,t>\\]", "", rhs).strip()
        if not rhs or "#" in rhs or re.search(r'(?<!@)"[^"]{2,}"', rhs) or re.search(r"@(?![\'\"])", rhs):
            continue
        if not re.fullmatch(r"[A-Za-z_0-9@'\"\s+\-*()]*(div [A-Za-z_0-9@'\"()]+)?", rhs):
            continue
        if name not in defs:
            defs[name] = (sec, rhs)
            order.append(name)
    return defs, order


defs, order = parse("pdftex-merged.web")
tex_defs, _ = parse("tex-merged.web")
xe_defs, xe_order = parse("xetex-merged.web")

# One layout for every flavor (DESIGN 4.7): pdfTeX's, with XeTeX's where
# XeTeX needs room. These are internal codes, so TeX's and pdfTeX's
# outputs do not change: tokens with 21-bit characters, XeTeX's two
# command codes in its places (pdfTeX's own after them), its glue and
# token list parameters, 256 math families, and the codes that must not
# be a character (`\span`, `\cr`, `\noexpand`'s marker).
XETEX_LAYOUT = """
cs_token_flag max_char_val left_brace_token left_brace_limit right_brace_token
right_brace_limit math_shift_token tab_token out_param_token space_token
letter_token other_token match_token end_match_token protected_token
XeTeX_math_given last_item max_non_prefixed_command toks_register assign_toks
assign_int assign_dimen assign_glue assign_mu_glue assign_font_dimen
assign_font_int set_aux set_prev_graf set_page_dimen set_page_int
set_box_dimen set_shape def_code XeTeX_def_code def_family set_font def_font
register max_internal advance multiply divide prefix let shorthand_def
read_to_cs def set_box hyph_data set_interaction
XeTeX_linebreak_skip_code thin_mu_skip_code med_mu_skip_code
thick_mu_skip_code glue_pars XeTeX_inter_char_loc etex_toks
number_math_families number_math_fonts math_font_biggest script_size
script_script_size backed_up_char inserted macro output_text every_par_text
every_math_text every_display_text every_hbox_text every_vbox_text
every_job_text every_cr_text mark_text inter_char_text
too_big_char biggest_usv too_big_usv number_usvs special_char
no_expand_flag span_code cr_code
""".split()
# XeTeX's codes that have no place in pdfTeX's numbering get partex's
# own, past pdfTeX's (the cited section is still XeTeX's).
OWN = {
    "letterspace_font": "set_interaction+1",
    "pdf_copy_font": "set_interaction+2",
    "partoken_name": "set_interaction+3",
    "max_command": "set_interaction+3",
    "cat_code_base": "math_font_base+number_math_fonts",
    "suppress_fontnotfound_error_code": "etex_int_base+10",
    "XeTeX_linebreak_locale_code": "etex_int_base+11",
    "XeTeX_linebreak_penalty_code": "etex_int_base+12",
    "XeTeX_protrude_chars_code": "etex_int_base+13",
    "eTeX_state_code": "etex_int_base+14",
    "eTeX_states": "12",
    "XeTeX_int": "eTeX_expr+4",
    "XeTeX_first_expand_code": "job_name_code+1",
    "pic_file_code": "pdftex_last_extension_code+1",
    "pdf_file_code": "pdftex_last_extension_code+2",
    "glyph_code": "pdftex_last_extension_code+3",
    "XeTeX_input_encoding_extension_code": "pdftex_last_extension_code+4",
    "XeTeX_default_encoding_extension_code": "pdftex_last_extension_code+5",
    "XeTeX_linebreak_locale_extension_code": "pdftex_last_extension_code+6",
}
# XeTeX's names that are not numbers partex uses (labels, field macros,
# memory layouts).
XE_SKIP = set("""
check_next end_node_run collect_native collected not_exp ASCII_code
packed_ASCII_code cur_length slow_print native_glyph plane_and_fam_field
update_prev_p do_size_requests native_node_size glyph_node_size
native_glyph_info_size pic_node_size XeTeX_linebreak_skip
suppress_fontnotfound_error XeTeX_linebreak_locale XeTeX_linebreak_penalty
XeTeX_protrude_chars inter_char_val
""".split())
xetex_cite = {}
for n in XETEX_LAYOUT:
    sec, rhs = xe_defs[n]
    if n not in defs:
        order.append(n)
    defs[n] = (sec, rhs)
    xetex_cite[n] = sec
for n in xe_order:
    if n in defs or n in XE_SKIP or n.endswith("_state") or n.endswith("_mode") and "input_mode" not in n:
        continue
    if n in ("XeTeX_default_input_mode", "XeTeX_default_input_encoding", "XeTeX_hyphenatable_length"):
        continue
    order.append(n)
    defs[n] = xe_defs[n]
    xetex_cite[n] = xe_defs[n][0]
for n, rhs in OWN.items():
    sec = xe_defs[n][0] if n in xe_defs else defs[n][0]
    if n not in defs:
        order.append(n)
    defs[n] = (sec, rhs)
    if n in xe_defs:
        xetex_cite[n] = sec

def ids(rhs):
    rhs = re.sub(r'@"[0-9A-F]+|"."', "", rhs)
    return set(re.findall(rf"\b{ID}\b", rhs)) - {"div"}

known = set(defs)
changed = True
while changed:
    changed = False
    for n in list(known):
        if not ids(defs[n][1]) <= known:
            known.discard(n)
            changed = True

def conv(v):
    # WEB single-character strings denote character codes; `""""` is `"`.
    v = v.replace('""""', "34")
    v = re.sub(r'"(.)"', lambda m: str(ord(m.group(1))), v)
    v = re.sub(r"@'([0-7]+)", lambda m: "0o" + m.group(1), v)
    v = re.sub(r'@"([0-9A-F]+)', lambda m: "0x" + m.group(1), v)
    v = re.sub(r"\bdiv\b", "/", v)
    v = re.sub(r"\b0x[0-9A-F]+\b", lambda m: m.group(0).lower(), v)
    return re.sub(rf"\b(?!0x){ID}\b", lambda m: m.group(0).upper(), v)

out = [
    "//! Every numeric constant of the merged WEB source (`@d x=...`), in source",
    "//! order. Generated by `scripts/gen-webconsts.py`; do not edit by hand.",
    "//! The section comments are where tex.web (or else pdftex.web) defines each",
    "//! name; values are pdfTeX's, whose layout is a superset of TeX's.",
    "",
    "#![allow(dead_code, unused_parens, clippy::identity_op, clippy::eq_op, clippy::double_parens)]",
    "",
]
for n in order:
    if n in known:
        sec, rhs = defs[n]
        if n in xetex_cite:
            cite = f"`XeTeX` §{xetex_cite[n]}"
        elif n in tex_defs:
            cite = f"§{tex_defs[n][0]}"
        else:
            cite = f"pdfTeX §{sec}"
        out.append(f"/// {cite}")
        out.append(f"pub const {n.upper()}: i32 = {conv(rhs)};")
out += [
    "",
    "/// The bits of a character in a token (`XeTeX`'s 21; DESIGN 4.7).",
    "pub const CHAR_BITS: u32 = 21;",
    "/// The character bits of a character token.",
    "pub const CHAR_MASK: i32 = MAX_CHAR_VAL - 1;",
    "",
    "/// The token of character `chr` with command code `cmd` (§289).",
    "#[inline]",
    "#[must_use]",
    "pub const fn char_token(cmd: i32, chr: i32) -> i32 {",
    "    cmd * MAX_CHAR_VAL + chr",
    "}",
    "",
    "/// The command code of a character token (`t < CS_TOKEN_FLAG`).",
    "#[inline]",
    "#[must_use]",
    "pub const fn tok_cmd(t: i32) -> i32 {",
    "    t >> CHAR_BITS",
    "}",
    "",
    "/// The character of a character token (`t < CS_TOKEN_FLAG`).",
    "#[inline]",
    "#[must_use]",
    "pub const fn tok_chr(t: i32) -> i32 {",
    "    t & CHAR_MASK",
    "}",
]
(root / "crates/partex-engine/src/web.rs").write_text("\n".join(out) + "\n")
print(f"{len(known)} constants")
