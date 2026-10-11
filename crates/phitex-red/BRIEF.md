# Writer brief (every agent)

You are one of 26 writers of a NEW TeX engine, crate `crates/phitex-red` in
the worktree /home/lambda/code/tmp/red (branch red). FIRST read in full:
CONSTRUCTS.md, and src/{lib,tok,val,item,term,env,ctx,prim}.rs (the contract),
and the stubs of the modules you call.

RULES (absolute):
- ONE SHOT: type 100% of your slice as complete, real code. Do NOT run cargo
  (no build/check/test), do not run anything, do not commit. Only read files
  and write your own files. No `todo!()` left in your files.
- Only edit the files you own. Do not edit the contract or other agents'
  files. Need something not there? Call it by the name you need anyway and
  list it under NEEDS in your final report. Keep the pub signatures already
  in your stubs (others call them); add more pub items freely.
- Do NOT read or copy anything from crates/phitex-flow or any other engine
  code in this repo. Semantics from your knowledge of tex.web / etex / pdftex.
  Execution model = CONSTRUCTS.md: graph reduction of pending(env; input)
  thunks. BANNED: mutable machine state, input/save stacks, semantic nest
  array, cond_ptr/skip mode, φ, re-running text.
- Primitives take `&mut dyn Ctx`, scan their own arguments through scan.rs,
  and change things only through Ctx (define/emit/push/open_*/close_*).
  The node records what it consumed and read: it is a pure function of them.
- Exact to TeX's observable output (arithmetic, rounding, item order). Errors:
  `crate::diag::error(ctx, msg, help)` plus TeX's recovery.
- Register your primitives in your module's `register(&mut Registry)`.
- Final report (short): files, line counts, NEEDS (exact signatures assumed),
  known gaps.
