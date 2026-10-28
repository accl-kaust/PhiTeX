// How a wasm host hands `\write18` to the Pyodide runner (runner.mjs).
//
// The engine decides which command runs and how it is quoted (web2c's
// `runsystem`, `partex_core::shell`) and calls `Host::system(command,
// inputs)` only for an allowed one. A wasm host (the extension's
// `MemHost`, core-partex/src/lib.rs) implements it with two imports, as
// its Shelf imports `fetch` and `fetch_copy` do (no call back into the
// module while it runs): the files the command may read go out framed as
// the `ph_*` ABI frames files (`u32 n, (u32 len, name, u32 len, bytes) ×
// n`, little-endian), and what it did comes back as `i32 status`, then
// the files it made or changed, framed, then the names it removed,
// framed with no bytes:
//
//   #[link(wasm_import_module = "phitex")]
//   unsafe extern "C" {
//       /// Run `cmd` over `files`: the length of what it did, then copied by `system_copy`.
//       fn system_run(cmd: *const u8, cmd_len: usize, files: *const u8, files_len: usize) -> u32;
//       fn system_copy(dst: *mut u8);
//   }
//
//   impl Host for MemHost {
//       fn system(&mut self, command: &[u8], inputs: &[(Vec<u8>, Arc<[u8]>)]) -> Option<Ran> {
//           // the project's files, what the job wrote, and the build's own
//           // version of the job's files where the command runs, last
//           let mut files: BTreeMap<&[u8], &[u8]> = BTreeMap::new();
//           files.extend(self.files.iter().map(|(n, c)| (&n[..], &c[..])));
//           files.extend(self.written.iter().map(|(n, c)| (&n[..], &c[..])));
//           files.extend(inputs.iter().map(|(n, c)| (&n[..], &c[..])));
//           let framed = frame(&files);
//           let n = unsafe { system_run(command.as_ptr(), command.len(), framed.as_ptr(), framed.len()) };
//           let mut out = vec![0u8; n as usize];
//           unsafe { system_copy(out.as_mut_ptr()) };
//           let status = i32::from_le_bytes(out.get(..4)?.try_into().ok()?);
//           let (wrote, rest) = parse_framed(&out[4..])?;      // parse_assets, giving the rest
//           let (removed, _) = parse_framed(rest)?;
//           let wrote: Vec<(Vec<u8>, Arc<[u8]>)> = wrote.into_iter().collect();
//           let removed: Vec<Vec<u8>> = removed.into_keys().collect();
//           for (n, c) in &wrote {
//               self.written.insert(n.clone(), c.to_vec());   // read_file finds them
//           }
//           for n in &removed {
//               self.written.remove(n);
//           }
//           // (an SSA build stores them, DESIGN 3.7, "Commands"; after a
//           // link, `ssa::removed_files` says which the job leaves removed)
//           Some(Ran { status, wrote, removed })
//       }
//   }
//
// The worker that instantiates the engine adds the imports, with the
// runner loaded before the first build (`system` is synchronous):
//
//   const runner = await createMintedRunner({ loadPyodide, wheels });
//   phitex: { ...shelfImports(() => memory), ...systemImports(() => memory, runner) }
//
// The engine's parameters must say restricted shell escape with TeX
// Live's list (`Params { shell_escape: true, restricted_shell: true,
// shell_escape_commands: shell::command_list(b"bibtex,...,latexminted,...")
// }`), as `pdflatex` has by default; else `\write18` is `disabled` and
// minted stops with "minted v3+ executable is not installed".

const dec = new TextDecoder();
const enc = new TextEncoder();

/** `u32 n, (u32 len, name, u32 len, bytes) × n` → [name, bytes] pairs. */
export function unframe(bytes) {
  const v = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let at = 0;
  const u32 = () => {
    const x = v.getUint32(at, true);
    at += 4;
    return x;
  };
  const out = [];
  for (let n = u32(); n > 0; n--) {
    const k = u32();
    const name = dec.decode(bytes.subarray(at, at + k));
    at += k;
    const len = u32();
    out.push([name, bytes.slice(at, at + len)]);
    at += len;
  }
  return out;
}

/** [name, bytes] pairs → `u32 n, (u32 len, name, u32 len, bytes) × n`. */
export function frame(files) {
  const parts = [...files].map(([n, b]) => [enc.encode(n), b]);
  const size = 4 + parts.reduce((s, [n, b]) => s + 8 + n.length + b.length, 0);
  const out = new Uint8Array(size);
  const v = new DataView(out.buffer);
  let at = 0;
  v.setUint32(at, parts.length, true);
  at += 4;
  for (const [n, b] of parts) {
    v.setUint32(at, n.length, true);
    out.set(n, at + 4);
    at += 4 + n.length;
    v.setUint32(at, b.length, true);
    out.set(b, at + 4);
    at += 4 + b.length;
  }
  return out;
}

/** What a command did, as `system_copy` hands it over. */
export function frameRan({ status, wrote, removed }) {
  const a = frame(wrote);
  const b = frame(removed.map((n) => [n, new Uint8Array(0)]));
  const out = new Uint8Array(4 + a.length + b.length);
  new DataView(out.buffer).setInt32(0, status, true);
  out.set(a, 4);
  out.set(b, 4 + a.length);
  return out;
}

/**
 * The `system_run` and `system_copy` imports over `runner`; `memory()`
 * gives the engine's memory.
 */
export function systemImports(memory, runner) {
  let pending = new Uint8Array(0);
  const bytes = (p, n) => new Uint8Array(memory().buffer, p >>> 0, n);
  return {
    system_run(cmdPtr, cmdLen, filesPtr, filesLen) {
      const command = dec.decode(bytes(cmdPtr, cmdLen).slice());
      const files = unframe(bytes(filesPtr, filesLen).slice());
      pending = frameRan(runner.system(command, files));
      return pending.length;
    },
    system_copy(dst) {
      bytes(dst, pending.length).set(pending);
      pending = new Uint8Array(0);
    },
  };
}
