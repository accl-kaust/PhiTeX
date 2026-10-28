// Run phitex-wasm (crates/wasm, built for wasm32-wasip1-threads by
// scripts/wasm.sh) in Node: WASI preview 1 for files, the environment and
// the clock (node:wasi: the directory it runs in and the system's TeX
// directories, `preopens` below; run it through scripts/sandbox), and
// wasi-threads' `thread-spawn`
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

import { accessSync, constants, readFileSync, writeSync } from "node:fs";
import { WASI } from "node:wasi";
import { Worker, isMainThread, workerData } from "node:worker_threads";

// (the limits crates/wasm/build.rs links with: the maximum from the start,
// 65535 pages, so the memory never grows)
const PAGES_INITIAL = 65535;
const PAGES_MAX = 65535;

// The file system as the program sees it: the directory Node runs in as
// the root itself, so that a relative path (WASI's are taken from "/", it
// has no current directory) is one from here; and under their own paths
// only the directories it reads TeX's files from, PHITEX_WASM_DIRS
// (colon-separated; by default the system's: /usr, /etc, /var, /opt,
// where a TeX installation, its texmf.cnf and its generated files are),
// each one it can open. Every directory at the root was preopened before:
// under a sandbox one it cannot open (/root, a mount it may not read) ends
// the WASI start with UVWASI_EACCES. The longest matching preopen wins.
function preopens() {
  const p = { "/": process.cwd() };
  const dirs = (process.env.PHITEX_WASM_DIRS ?? "/usr:/etc:/var:/opt").split(":");
  for (const d of dirs) {
    if (!d.startsWith("/") || d === "/") continue;
    try {
      accessSync(d, constants.R_OK | constants.X_OK);
      p[d] = d;
    } catch {
      // (absent or unreadable: not the program's)
    }
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
