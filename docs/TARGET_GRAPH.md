# The target graph

What the def-use graph of a document must look like, fixture by fixture,
and the test that asserts it. Acceptance for every scheduling change is
this edge set, not a chain fraction.

## Nodes

One node per source command with a text position. Expansion runs inline
in the node that reads the token: a macro body, a conditional, `\the`,
`\csname` never become nodes. A run continues through the whole token
frame it is expanding and ends only at a boundary:

- the end of its source slice;
- a paragraph end (`\par`, empty line, display start/end): the run goes
  on in the same node, but from there its region anchor is its own node id
  (the boundary rule, `ops.rs may_define_field`);
- `\input` / `\endinput` (a file-rest child `["f", …]`);
- a page-step marker (a light `PageStep` child) and the output routine
  (one node, however many conditionals it contains).

Children are created only for those boundaries. There are no `["t", …]`
children: a token frame left over at the end of a run is a bug in the
run, not a node. A node's args carry `[frames, top, anchor, boundary,
ifdepth]`; `anchor` is never 0: a child inherits its parent's anchor, a
node after a paragraph end uses its own id. (At 2d820888 every
positionless child had anchor 0, so all paragraphs of `pages` shared one
cell `par:0:0` and chained through it: 24 of its 59 chain links.)

## Edges allowed

Writing `A <- B` for "A reads what B defined".

| reader | may read | from |
|---|---|---|
| text node T in paragraph k | `par:<A_k>:<vl>` | the node S_k that started the paragraph (new_graf) |
| text node T | regime cells (`cat:`, `cs:`, `font`, `param:`, `count:`) | the last node before T that changed them: a fan-out, not a chain |
| paragraph end E_k | `items` of region A_k | the T's of paragraph k (one fan-in) |
| page step P_j | `items` since P_{j-1} | the E_k's and vertical material (one fan-in) |
| output routine O_j | `pg:`, `page:state`, the page box | P_j and O_{j-1} |
| `\end` | pages | the last O |

Forbidden, and asserted by the test:

- a text node of paragraph k reading anything paragraph k-1 defined,
  other than a regime cell it changed (`Changed par:` between regions);
- any node reading `par:0:…`;
- a node woken by a node that answered `Any` when the text of the
  waiter is plain letters, spaces and other characters;
- a `Changed if:` link between two different source commands (the
  conditional is inline);
- an `items` read by anything other than a paragraph end, a page step,
  a box end or an alignment end.

## Heights

With those edges the critical path is: paragraph (3) + page step (1) +
output routine chain (2 per page) + `\end`. For `pages` (10 pages): about
25 of 456 nodes, fraction < 0.06. For `story`: < 15. For `hyphens`: < 15.
The test asserts `height <= 4 + 3 * pages`, per fixture.

## The test

`chains` prints, per fixture, every link with its cause and the two
nodes' source positions; it fails on any forbidden link above, on any
`par:0:` field, on `height > 4 + 3 * pages`, and on an evaluation with no
page step. Bytes identical to pdfTeX remain necessary; they are not the
acceptance.
