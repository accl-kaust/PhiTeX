// Run phitex-wasm (crates/wasm, built for wasm32-wasip1-threads by
// scripts/wasm.sh) in Node: WASI preview 1 for files, the environment and
// the clock (node:wasi, the whole file system as the program sees it:
// run it through scripts/sandbox), and wasi-threads' `thread-spawn`
// answered with a Node worker per thread, each instantiating the same
// module on the same shared memory and entering `wasi_thread_start`.
// No wasm-bindgen and no `unsafe` in the program: std's threads are
// these workers.
//
//   cd doc-dir && node .../scripts/wasm-run.mjs .../phitex-wasm.wasm [arguments] doc.tex
//
// The exit status is the program's. A worker that traps (a panic: the
// target aborts) ends the whole process (SIGTERM), since the main thread,
// blocked on the engine's locks, cannot hear of it.

import { readdirSync, readFileSync, writeSync } from "node:fs";
import { WASI } from "node:wasi";
import { Worker, isMainThread, workerData } from "node:worker_threads";

// (the limits crates/wasm/build.rs links with: 64 MiB at first, up to 4 GiB)
const PAGES_INITIAL = 1024;
const PAGES_MAX = 65536;

// The file system as the program sees it: every directory at the root
// under its own path, and the directory Node runs in as the root itself,
// so that a relative path (WASI's are taken from "/", it has no current
// directory) is one from here. The longest matching preopen wins.
function preopens() {
  const p = { "/": process.cwd() };
  for (const d of readdirSync("/", { withFileTypes: true })) {
    if (d.isDirectory() || d.isSymbolicLink()) p[`/${d.name}`] = `/${d.name}`;
  }
  return p;
}

function wasiFor(args, env) {
  return new WASI({
    version: "preview1",
    args,
    env,
    preopens: preopens(),
    returnOnExit: true,
  });
}

function imports(wasi, memory, shared) {
  return {
    ...wasi.getImportObject(),
    env: { memory },
    wasi: {
      "thread-spawn": (startArg) => {
        const tid = Atomics.add(shared.next, 0, 1);
        if (process.env.PHITEX_WASM_VERBOSE) writeSync(2, `phitex-wasm: thread ${tid} started\n`);
        new Worker(new URL(import.meta.url), {
          workerData: { ...shared.data, tid, startArg },
        });
        return tid;
      },
    },
  };
}

function shared(module, memory, args, env, next) {
  return { next, data: { module, memory, args, env, nextBuf: next.buffer } };
}

if (isMainThread) {
  const [wasmPath, ...rest] = process.argv.slice(2);
  if (!wasmPath) {
    console.error("usage: node scripts/wasm-run.mjs phitex-wasm.wasm [arguments]");
    process.exit(2);
  }
  const module = new WebAssembly.Module(readFileSync(wasmPath));
  const memory = new WebAssembly.Memory({
    initial: PAGES_INITIAL,
    maximum: PAGES_MAX,
    shared: true,
  });
  // (the threads' ids, from 1: the main thread is 0)
  const next = new Int32Array(new SharedArrayBuffer(4));
  next[0] = 1;
  const args = ["phitex-wasm", ...rest];
  const env = { ...process.env };
  const wasi = wasiFor(args, env);
  const instance = new WebAssembly.Instance(
    module,
    imports(wasi, memory, shared(module, memory, args, env, next)),
  );
  // (node:wasi takes the memory from the exports; the module imports it)
  const code = wasi.start({ exports: { ...instance.exports, memory } });
  process.exit(code ?? 0);
} else {
  const { module, memory, args, env, nextBuf, tid, startArg } = workerData;
  const next = new Int32Array(nextBuf);
  const wasi = wasiFor(args, env);
  const s = shared(module, memory, args, env, next);
  const instance = new WebAssembly.Instance(module, imports(wasi, memory, s));
  wasi.initialize({ exports: { memory } });
  try {
    instance.exports.wasi_thread_start(tid, startArg);
  } catch (e) {
    writeSync(2, `phitex-wasm: thread ${tid} trapped: ${e?.stack ?? e}\n`);
    process.kill(process.pid, "SIGTERM");
  }
}
