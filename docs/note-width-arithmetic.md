# Dimensions past 2^31 sp: what pdfTeX does, what PhiTeX does

Open, v1.0. `Scaled` is `i32`; a sum of dimensions past 2^31 sp (32768pt)
wraps in release and panics where overflow checks are on (`pack.rs` was where a
test found it). The test `pdftex::widths_past_2_31_sp_as_pdftex`, ignored, holds
the case; see "Sweep" below for whether a real document reaches it.

## The oracle's integer is 64 bits, and that is its build's choice

`tex.web`: `scaled = integer`. web2c `w2c/config.h`: `integer` is `int` when
`SIZEOF_LONG > 4 && !NO_DUMP_SHARE`, else `long`. `texk/web2c/configure.ac`
defines `NO_DUMP_SHARE` only for `--disable-dump-share`. Arch's `texlive-bin`
PKGBUILD passes `--disable-dump-share`, so the CI image's pdfTeX (TeX Live 2026.1,
Arch) has a 64-bit `integer`: the results below agree. `texmfmem.h`'s
`memory_word` holds `integer cint`, so box dimensions in `mem` keep all of it.

A TeX Live built with the default (dump-share on) has a 32-bit `integer`. That is
read from the sources only: no such binary was run. If so, two pdfTeX builds
differ on these documents, and "identical to pdfTeX" here means the oracle's.

## Measured on the oracle (`pdftex -ini`, CI image)

| Input | pdfTeX | Rule |
|---|---|---|
| `\hbox{}` of 3 x `\hskip16000pt`, `\the\wd0` | 48000.0pt | hpack's `x` has no bound |
| 40 x the same | 640000.0pt | |
| `\count1=\wd0` | 3145728000 | past 2^31, no error |
| `\dimen0=\wd0` | "Dimension too large", 16383.99998pt | scanning a dimension checks 2^30 |
| `\wd0=48000pt` | "Dimension too large", 16383.99998pt | the same |
| `\dimen1=16000pt \advance\dimen1 by 16000pt` | 32000.0pt, no error | |
| `\dimen1=10000pt \multiply\dimen1 by 3` | "Arithmetic overflow", 10000.0pt | `mult_and_add`'s bound |
| `\hbox to 100pt` over 48000pt | Overfull (47900.0pt too wide), width 100pt | natural width whole |
| `\vbox to 100pt` over 48000pt | Overfull (47900.0pt too high), height 100pt | the same in vpack |
| `\shipout` of a 48001pt box | "Huge page cannot be shipped out" | `width + h_offset > max_dimen` |
| `\shipout` of `\hbox to 100pt` over 48001pt | page written | positions past 2^31 in the writer |

| `\count1=2147483640`, then 5 x `\advance\count1 by 3` | 2147483655, no error | the sum has no bound; a `\count` holds all 64 bits |
| The same in a loop until `\ifnum\count1>0` is false | does not end (killed after 100 minutes) | the register never turns negative |

PhiTeX on the `\advance` row: -2147483641 (`do_register_command` wraps, and its
comment says web2c does; the oracle's build does not). `Run::counted_loop`
leaves any step past `i32` to the interpreter, so this fix changes it in one
place. Measured with `count_loop.tex` (accl 24140, oracle pdftex in the CI image;
XeTeX the same, an `\advance` past 2^31 prints the same digits).

PhiTeX on the first row: -17536.0pt. Glue totals (stretch, shrink) were only
measured to not disturb `\hbox to 100pt`; the last row's DVI bytes were not
compared with PhiTeX's.

## What a fix needs

The accumulators and the fields they land in hold 64 bits: box width, height,
depth, `\count` values (read from them, or stepped past 2^31 on their own), the writers' positions. Bounds stay where
TeX has them (scanning at 2^30, `mult_and_add`, Huge page), not new ones.
Changing the alias alone gives 122 compile errors in `phitex-typeset`, and the
engine has about 1000 lines that mention `i32`; node size and speed need an A/B.
