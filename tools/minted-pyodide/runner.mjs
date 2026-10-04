// A `\write18` runner for partex in the browser (or Node): latexminted,
// with Pygments, in Pyodide, over an in-memory file system the engine's
// files are copied into. It implements what a wasm host's
// `Host::system` hands to JavaScript (DESIGN 3.7, "Commands"):
//
//   const runner = await createMintedRunner({ loadPyodide, wheels });
//   const { status, wrote, removed } = runner.system(command, files);
//
// - `command`: the command as the engine decided it (web2c's
//   `runsystem`: in restricted mode, quoted, `latexminted 'batch' ...`).
//   It is split as `/bin/sh` would split it; only `latexminted` runs
//   (anything else: status 127, as a shell that cannot find it).
// - `files`: a Map (or [name, Uint8Array] pairs) of the job's files as
//   the build holds them, by the names TeX uses (relative to the job's
//   directory): copied into Pyodide's `/work` where they differ.
// - `wrote`: a Map name -> Uint8Array of the files the command made or
//   changed under `/work`, for the host to keep (and, in an SSA build,
//   for the engine to store); `removed`: the names of those it removed.
//
// `system` is synchronous (Pyodide runs Python synchronously once it is
// loaded), as the engine's call into the host is: load the runner before
// the build, in the worker the engine runs in.
//
// latexminted reads TeX Live's configuration through `latexrestricted`,
// which runs `kpsewhich`; Pyodide has no processes, so `subprocess.run`
// is answered here for `kpsewhich --var-value VAR` (TeX Live 2026's
// texmf.cnf values that matter) and `kpsewhich FILE` (a file in
// `/work`), and `SELFAUTOLOC` names a `kpsewhich` placeholder.

const WORK = '/work';
const BIN = '/texlive/bin';

// TeX Live 2026's texmf.cnf, as `kpsewhich --var-value` answers it.
const TEXMF_VARS = {
  openin_any: 'a',
  openout_any: 'p',
  shell_escape: 'p',
  shell_escape_commands:
    'bibtex,bibtex8,extractbb,gregorio,kpsewhich,l3sys-query,latexminted,makeindex,memoize-extract.pl,memoize-extract.py,repstopdf,r-mpost,texosquery-jre8,',
  TEXMFOUTPUT: '',
  TEXMFHOME: '/texlive/texmf-home',
};

const SETUP = `
import os, sys, shlex, subprocess, types
os.chdir(${JSON.stringify(WORK)})
os.environ['SELFAUTOLOC'] = ${JSON.stringify(BIN)}
os.environ['SELFAUTODIR'] = '/texlive'
os.environ['SELFAUTOPARENT'] = '/'
os.environ['TEXSYSTEM'] = 'texlive'
_vars = ${JSON.stringify(TEXMF_VARS)}

def _run(cmd, *args, **kw):
    # (kpsewhich only: latexrestricted runs nothing else)
    name = os.path.basename(cmd[0])
    if name != 'kpsewhich':
        raise PermissionError(cmd[0])
    if len(cmd) == 3 and cmd[1] == '--var-value':
        out = _vars.get(cmd[2], '')
    else:
        f = cmd[-1]
        out = f if os.path.isfile(f) else ''
    return subprocess.CompletedProcess(cmd, 0 if out else 1, (out + '\\n').encode() if out else b'', b'')
subprocess.run = _run

def _snapshot(reset):
    # (with reset, every file's time is set to 0 first: a file the
    # command writes, however soon, has another)
    out = {}
    for root, dirs, files in os.walk('.'):
        dirs[:] = [d for d in dirs if not d.startswith('.')]
        for f in files:
            p = os.path.join(root, f)[2:]
            if reset:
                os.utime(p, (0, 0))
            st = os.stat(p)
            out[p] = [st.st_size, st.st_mtime, st.st_ino]
    return out

def _system(command):
    argv = shlex.split(command)
    if not argv or argv[0] != 'latexminted':
        return 127 << 8
    from latexminted.cmdline import main
    old = sys.argv
    sys.argv = argv
    try:
        main()
        code = 0
    except SystemExit as e:
        code = e.code if isinstance(e.code, int) else (0 if e.code is None else 1)
    except BaseException as e:
        print(f'latexminted: {e!r}', file=sys.stderr)
        code = 1
    finally:
        sys.argv = old
    return (code & 0xff) << 8
`;

function mkdirs(FS, path) {
  let at = '';
  for (const part of path.split('/').filter(Boolean)) {
    at += '/' + part;
    if (!FS.analyzePath(at).exists) FS.mkdir(at);
  }
}

function sameBytes(a, b) {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

/**
 * Load Pyodide with latexminted and its dependencies.
 * @param {object} o
 * @param {Function} o.loadPyodide  pyodide's `loadPyodide` (from the npm
 *   package `pyodide`, or `pyodide.js` in the browser)
 * @param {object} [o.pyodideOptions]  passed to `loadPyodide` (`indexURL`)
 * @param {Array<[string, Uint8Array]>} o.wheels  the four pure-Python
 *   wheels, by file name: latexminted, latexrestricted, latex2pydata,
 *   pygments (TeX Live ships them in `scripts/minted/`; or PyPI)
 */
export async function createMintedRunner({ loadPyodide, pyodideOptions = {}, wheels }) {
  const pyodide = await loadPyodide({ ...pyodideOptions, stdout: () => {}, stderr: () => {} });
  const FS = pyodide.FS;
  // (pure-Python wheels: unpacked into site-packages, no micropip)
  const site = pyodide.runPython('import site; site.getsitepackages()[0]');
  for (const [, bytes] of wheels) {
    pyodide.unpackArchive(bytes, 'zip', { extractDir: site });
  }
  pyodide.runPython('import importlib; importlib.invalidate_caches()');
  mkdirs(FS, WORK);
  mkdirs(FS, BIN);
  mkdirs(FS, TEXMF_VARS.TEXMFHOME);
  FS.writeFile(`${BIN}/kpsewhich`, '#!/bin/sh\n');
  FS.chmod(`${BIN}/kpsewhich`, 0o755);
  pyodide.runPython(SETUP);
  const snapshot = pyodide.globals.get('_snapshot');
  const run = pyodide.globals.get('_system');
  // (what the last sync wrote, so unchanged files are not copied again)
  const copied = new Map();

  function system(command, files) {
    for (const [name, bytes] of files instanceof Map ? files.entries() : files) {
      const path = `${WORK}/${name.replace(/^\.\//, '')}`;
      const last = copied.get(path);
      if (last && sameBytes(last, bytes) && FS.analyzePath(path).exists) continue;
      mkdirs(FS, path.slice(0, path.lastIndexOf('/')));
      FS.writeFile(path, bytes);
      copied.set(path, bytes);
    }
    const before = snapshot(true).toJs({ dict_converter: Object.fromEntries });
    const status = run(command);
    const after = snapshot(false).toJs({ dict_converter: Object.fromEntries });
    const wrote = new Map();
    for (const [name, st] of Object.entries(after)) {
      const was = before[name];
      if (was && was.every((x, i) => x === st[i])) continue;
      const bytes = FS.readFile(`${WORK}/${name}`);
      wrote.set(name, bytes);
      copied.set(`${WORK}/${name}`, bytes);
    }
    const removed = Object.keys(before).filter((name) => !(name in after));
    for (const name of removed) copied.delete(`${WORK}/${name}`);
    return { status, wrote, removed };
  }

  return { system, pyodide };
}
