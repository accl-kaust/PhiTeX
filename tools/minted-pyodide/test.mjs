// Check the Pyodide runner against the installed latexminted: the same
// commands over the same inputs (the data files minted wrote before each
// call, captured from two pdflatex runs of tests/e2e/minted.tex: a
// directory of `N/args` and `N/*.data.minted`, `fixture/` by default)
// must make the same files, byte for byte, in the same order of calls.
// The calls go through `host-glue.mjs`'s imports, as a wasm host makes
// them. After `npm install` here:
//
//   scripts/sandbox node tools/minted-pyodide/test.mjs [CAPTURE_DIR]

import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { loadPyodide } from 'pyodide';
import { createMintedRunner } from './runner.mjs';
import { frame, systemImports, unframe } from './host-glue.mjs';

const WHEELS = '/usr/share/texmf-dist/scripts/minted';
const cap = process.argv[2] ?? new URL('fixture', import.meta.url).pathname;

const wheels = fs
  .readdirSync(WHEELS)
  .filter((f) => f.endsWith('.whl'))
  .map((f) => [f, new Uint8Array(fs.readFileSync(path.join(WHEELS, f)))]);
let t = performance.now();
const runner = await createMintedRunner({ loadPyodide, wheels });
console.log(`pyodide and latexminted loaded in ${(performance.now() - t).toFixed(0)} ms`);

// web2c's quoting of an allowed command (`shell_cmd_is_allowed`)
const quote = (args) => ['latexminted', ...args.map((a) => `'${a}'`)].join(' ');

function walk(dir, rel = '', out = new Map()) {
  for (const e of fs.readdirSync(path.join(dir, rel), { withFileTypes: true })) {
    const r = rel ? `${rel}/${e.name}` : e.name;
    if (e.isDirectory()) walk(dir, r, out);
    else out.set(r, fs.readFileSync(path.join(dir, r)));
  }
  return out;
}

// the call as a wasm host makes it: the command and the files framed in
// the module's memory, through the `system_run` and `system_copy` imports
const memory = new WebAssembly.Memory({ initial: 512 });
const imports = systemImports(() => memory, runner);
function viaImports(cmd, inputs) {
  const m = new Uint8Array(memory.buffer);
  const c = new TextEncoder().encode(cmd);
  const f = frame(inputs);
  m.set(c, 0);
  m.set(f, c.length);
  const n = imports.system_run(0, c.length, c.length, f.length);
  const at = c.length + f.length;
  imports.system_copy(at);
  const out = new Uint8Array(memory.buffer).slice(at, at + n);
  const status = new DataView(out.buffer).getInt32(0, true);
  const wrote = unframe(out.subarray(4));
  return { status, wrote: new Map(wrote) };
}

const native = fs.mkdtempSync(path.join(os.tmpdir(), 'minted-native-'));
const steps = fs
  .readdirSync(cap)
  .map(Number)
  .sort((a, b) => a - b);
let failures = 0;
for (const n of steps) {
  const args = fs.readFileSync(path.join(cap, `${n}`, 'args'), 'utf8').trim().split(/\s+/);
  const inputs = new Map(
    fs
      .readdirSync(path.join(cap, `${n}`))
      .filter((f) => f.endsWith('.data.minted'))
      .map((f) => [f, new Uint8Array(fs.readFileSync(path.join(cap, `${n}`, f)))]),
  );
  const cmd = quote(args);
  t = performance.now();
  const { status, wrote } = viaImports(cmd, inputs);
  const ms = performance.now() - t;
  // the same, natively
  for (const [f, b] of inputs) fs.writeFileSync(path.join(native, f), b);
  const r = spawnSync('/bin/sh', ['-c', cmd], {
    cwd: native,
    env: { ...process.env, SELFAUTOLOC: '/usr/bin', SELFAUTODIR: '/usr', SELFAUTOPARENT: '/' },
  });
  // the trees after the call: the same files, the same bytes
  const mine = new Map();
  const ls = runner.pyodide.runPython(
    `import os\n[os.path.join(r, f)[2:] for r, d, fs in os.walk('.') for f in fs]`,
  ).toJs();
  for (const f of ls) mine.set(f, runner.pyodide.FS.readFile(`/work/${f}`));
  const theirs = walk(native);
  const names = [...new Set([...mine.keys(), ...theirs.keys()])].sort();
  const diff = names.filter(
    (f) => !mine.has(f) || !theirs.has(f) || Buffer.compare(Buffer.from(mine.get(f)), theirs.get(f)) !== 0,
  );
  console.log(
    `${cmd}: status ${status} (native ${r.status << 8}), ${ms.toFixed(0)} ms, wrote [${[...wrote.keys()].join(', ')}]` +
      (diff.length ? `; DIFFERENT: ${diff.join(', ')}` : '; identical trees'),
  );
  if (diff.length || status !== r.status << 8) failures++;
}
process.exit(failures ? 1 : 0);
