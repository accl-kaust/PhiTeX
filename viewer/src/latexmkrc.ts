// A project's latexmk rc files, as the core's open takes them (docs/latexmkrc.md):
// the root's `.latexmkrc`, else `latexmkrc`, as Overleaf reads it, and every
// file one of them loads with `do` or `require`. The core reads them (as data,
// never as Perl run); the session gives them, and reopens when one changes.

import type { Diagnostic } from "./diagnostics.ts";

/** The project's rc files by path, the root's first and what they load (none: the project has no rc file); `files` the project's texts, or a reader of its paths. */
export function latexmkFiles(files: Record<string, string> | ((path: string) => string | undefined)): Record<string, string> | undefined {
  const get = typeof files === "function" ? files : (p: string) => (Object.hasOwn(files, p) ? files[p] : undefined);
  const out: Record<string, string> = {};
  const read = (path: string) => {
    const text = Object.hasOwn(out, path) ? undefined : get(path);
    if (text === undefined) return;
    out[path] = text;
    // (a relative name is the working directory's, the project's root, only
    // from ./ or ../, as Perl takes it; a bare one the core says is not found.
    // ../ out of the project: not given, the core says so)
    for (const m of text.matchAll(/^[^#\n]*?\b(?:do|require)\s*\(?\s*(["'])(\.\.?\/[^"'\n]+)\1/gm)) {
      const p = normal(m[2]);
      if (p) read(p);
    }
  };
  read(get(".latexmkrc") !== undefined ? ".latexmkrc" : "latexmkrc");
  return Object.keys(out).length ? out : undefined;
}

/** `./a/../b/c` as the project's path `b/c`; one out of the project, none. */
function normal(path: string): string | undefined {
  const parts: string[] = [];
  for (const p of path.split("/")) {
    if (p === "" || p === ".") continue;
    if (p !== "..") parts.push(p);
    else if (!parts.pop()) return undefined;
  }
  return parts.join("/") || undefined;
}

/** The core's `latexmkrc` event's notes ("FILE:LINE: why", or a line without a place) as the panel's diagnostics. */
export function latexmkNotes(notes: string[]): Diagnostic[] {
  return notes.map((n) => {
    const m = /^(?:\/compile\/|\.\/)?([^\s:][^:]*):(\d+): ([\s\S]*)$/.exec(n);
    return m
      ? { severity: "info", code: "latexmkrc", file: m[1], line: +m[2], message: `latexmkrc: ${m[3]}` }
      : { severity: "info", code: "latexmkrc", message: `latexmkrc: ${n}` };
  });
}
