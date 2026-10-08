// Packages: the files a build reads that the project doesn't have
// (`\usepackage{amsmath}`'s amsmath.sty, a class, a .def), found elsewhere
// and handed to the core as files. The core reports them (status JSON's
// `missing`); the session asks a PackageSource and `set_file`s what it
// finds. The source is the core host's (corehost.ts, shelf.ts: the
// extension's texmf/, the package cache, then Shelf), asked over the core's
// transport (`hostPackages`): in Overleaf a content script's fetch would be
// Overleaf's origin (CORS, its CSP). Only what a build reads is fetched. (A .sty is TeX PhiTeX runs, not the extension's
// JavaScript: the Web Store's remote-code rule is about the latter.)

/** Where packages come from. */
export interface PackageSource {
  /** Shown to the user: "TeX Live 2025", ... */
  readonly label: string;
  /** `name`'s text (as the core asked for it: "amsmath.sty") for `engine` (pdflatex, xelatex: which tree's file), or null: not there. */
  resolve(name: string, engine?: string): Promise<string | null>;
}

/**
 * What `resolve` gives for a binary file (a font's metrics) the source
 * handed to the core itself: fetched, but no text for the session to keep.
 */
export const DELIVERED = "\u0000delivered";

/**
 * Packages as the core's host resolves them (its `package` op), asked over
 * `core`, each name once (`cached`): a failure is an error, shown with why;
 * null is "not in TeX Live".
 */
export function hostPackages(core: { request(r: { op: "package"; name: string; engine?: string }): Promise<{ ok: boolean; error?: string; delivered?: boolean; text?: string | null }> }): PackageSource {
  return cached({
    label: "TeX Live 2026",
    resolve: (name, engine) =>
      core.request({ op: "package", name, engine }).then((r) => {
        if (!r.ok) throw new Error(r.error ?? "package download failed");
        return r.delivered ? DELIVERED : (r.text ?? null);
      }),
  });
}

/** No packages (PhiTeX runs no LaTeX yet). */
export const noPackages: PackageSource = { label: "none", resolve: async () => null };

/** What the session tells the view: packages being downloaded, and the ones not found. */
export interface PackageState {
  /** Being fetched now (empty: none). */
  loading: string[];
  /** Asked for, found nowhere. */
  unavailable: string[];
  source: string;
  /** Fetched so far (this project's, since it opened). */
  done?: string[];
  /** The core is building with what arrived (a build stops at the first file it lacks). */
  building?: boolean;
  /** Fetches that failed (the network, the package server), with why: asked again on the next build. */
  failed?: { name: string; error: string }[];
}

/** A name a TeX distribution might have (not an .aux, a .bbl, a .toc: the build's own). A .tex the project lacks is asked for too: shelf.ts sends Shelf only names it lists. */
export function isPackageFile(name: string): boolean {
  return /\.(sty|cls|clo|def|cfg|fd|ldf|ltx|tex|dfu|sto|lbx|bbx|cbx|dbx|mld|tikz|pgf|code\.tex|lua|ini|cmap|fontspec|tfm|vf|pfb|enc|map)$/i.test(name) && !name.includes("/");
}

/** `inner`, each name fetched once (in flight or done), however often it is asked for. */
export function cached(inner: PackageSource): PackageSource {
  const seen = new Map<string, Promise<string | null>>();
  return {
    label: inner.label,
    resolve(name, engine) {
      const k = `${engine ?? ""}\t${name}`;
      let p = seen.get(k);
      // (a failure is not remembered: asked again, it is fetched again)
      if (!p) seen.set(k, (p = inner.resolve(name, engine).catch((e) => (seen.delete(k), Promise.reject(e)))));
      return p;
    },
  };
}

/**
 * The package files `text` will read, by a scan of its source (not TeX: a
 * guess, which is fine, since a name it gets wrong is fetched for nothing,
 * or found missing by the build, as before): `\documentclass`,
 * `\LoadClass`, `\usepackage`, `\RequirePackage` (comma lists, options
 * skipped), `\input`/`\InputIfFileExists` of a name with an extension.
 */
export function referenced(text: string): string[] {
  const out = new Set<string>();
  const body = text.replace(/(^|[^\\])%.*$/gm, "$1");
  for (const m of body.matchAll(/\\(documentclass|LoadClass(?:WithOptions)?|usepackage|RequirePackage(?:WithOptions)?)\s*(?:\[[^\]]*\]\s*)?\{([^}]*)\}/g)) {
    const ext = /class/i.test(m[1]) ? ".cls" : ".sty";
    for (const n of m[2].split(",")) {
      const name = n.trim();
      if (/^[A-Za-z0-9@_.-]+$/.test(name)) out.add(name + ext);
    }
  }
  // (a font definition file: the fonts its shapes name, as metrics; their
  // packs bring the .vf and .pfb, so a family arrives in one round, not a
  // build per font: `<-> \\scale@macro name`, `<5-> s*[0.9] name`)
  if (/\\DeclareFontShape/.test(body))
    for (const m of body.matchAll(/<[^>]*>\s*(?:\\[A-Za-z@]+\s*|s\*\s*(?:\[[^\]]*\]\s*)?)*([A-Za-z][A-Za-z0-9-]*)(?![A-Za-z0-9-]*\/)/g))
      if (!/^(s?sub|gen|genb|sgen|fixed|sfixed|error|serror|warning)$/.test(m[1])) out.add(m[1] + ".tfm");
  for (const m of body.matchAll(/\\(?:input|InputIfFileExists|@input)\s*\{?([A-Za-z0-9@_-]+\.(?:sty|cls|clo|def|cfg|fd|ldf|tex|dfu))\}?/g)) out.add(m[1]);
  return [...out];
}
