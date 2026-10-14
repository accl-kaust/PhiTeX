# Paragraph-start lists: a partial evaluator over the actual meanings

A paragraph's start runs the primitive `\everypar` list in force (tex.web
§1091). Under LaTeX that is the kernel's `\g__para_standard_everypar_tl`
(expl3). It sets `\g_para_indent_box` (box 32) globally at every
paragraph, runs the `para/before` and `para/begin` hooks, and runs LaTeX's
own `\everypar` register (`\the\toks12`), which the hint pass already
follows abstractly (`pass::parlists`). The text harvest cannot read expl3
definitions, and a closure over the format's actual meanings is a
may-analysis. Through the hook machinery (preamble hooks, the
replacing-args branches, prop/seq mappings, rescans) it declares about a
dozen fields a real paragraph never defines. One of them is
`\endlinechar`, which every paragraph reads, so the paragraphs form a
serial chain (paras30 cp 6 → 38). For now a paragraph proves its
void→void `\global\setbox` of box 32 a no-op with one read of the field
(the net-effect publish, `runstate::State::published`). That costs one
link where an `Any` node precedes the paragraphs.

The exact answer is a small partial evaluator. It runs the primitive list
over the meanings in force at the paragraph: the format's state as the
definitions chain changes it, since `\begin{document}` turns the preamble
hooks into no-ops. It decides conditionals (`\if_cs_exist:w`,
`\if_meaning:w`, `\ifx`, flags) on known values, expands an empty hook to
nothing, and gives up (the may-closure) only where a value is unknown.
What it returns per paragraph start is the set of fields the start
defines, here box 32 alone. The parlists machine runs it as a base
program before the register's list. The paragraph then declares box 32
and defines it without reading, and value equality cuts off its readers.
Inputs it needs: the prelude's environment (`Cached::env`: meanings,
chardefs, registers) and the chain's own definitions, both available at
hint time.
