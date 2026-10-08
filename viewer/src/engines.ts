// The TeX engines a project may need, and which one ⚡ Instant runs it with.
// pdfLaTeX and XeLaTeX run; LuaLaTeX is listed so the settings, the
// per-project choice and the prompt are ready when its core is.

export type Engine = "pdflatex" | "xelatex" | "lualatex";
/** The setting: an engine, or "auto" (the project's own choice, else what its preamble needs). */
export type EngineChoice = "auto" | Engine;

export const ENGINES: Record<Engine, { label: string; ready: boolean }> = {
  pdflatex: { label: "pdfLaTeX", ready: true },
  xelatex: { label: "XeLaTeX", ready: true },
  lualatex: { label: "LuaLaTeX", ready: false },
};

/** The engine a main file's preamble needs, if not pdfLaTeX (null: pdfLaTeX will do). */
export function needs(main: string | undefined): Engine | null {
  if (!main) return null;
  const end = main.indexOf("\\begin{document}");
  // (comments out: a commented-out \usepackage{fontspec} needs nothing)
  const pre = (end < 0 ? main : main.slice(0, end)).replace(/(^|[^\\])%.*$/gm, "$1");
  if (/\\(directlua|luaexec)\b|\\usepackage(\[[^\]]*\])?\{[^}]*\b(luacode|luatexja|luaotfload|lua-visual-debug)\b/.test(pre)) return "lualatex";
  if (/\\usepackage(\[[^\]]*\])?\{[^}]*\b(fontspec|unicode-math|polyglossia|xeCJK|xltxtra|mathspec)\b/.test(pre)) return "xelatex";
  if (/\\documentclass(\[[^\]]*\])?\{(ctexart|ctexrep|ctexbook)\}/.test(pre)) return "xelatex";
  return null;
}

/** A build error that says another engine is needed (fontspec's, unicode-math's, …). */
export function errorNeeds(message: string): Engine | null {
  if (/LuaTeX is required|requires LuaTeX/i.test(message)) return "lualatex";
  if (/requires either XeTeX or|XeTeX or LuaTeX|(fontspec|unicode-math|polyglossia)\b.*(XeTeX|LuaTeX)/i.test(message)) return "xelatex";
  return null;
}

/** The engine to run: the project's own choice, else the setting, else (auto) what the preamble needs. */
export function resolve(setting: EngineChoice, project: Engine | undefined, main: string | undefined): Engine {
  return project ?? (setting !== "auto" ? setting : (needs(main) ?? "pdflatex"));
}

/** The stand-ins (common/shims/, the extension's shims/) a XeLaTeX project is approximated with under pdfLaTeX. */
export const SHIMS = ["fontspec.sty", "unicode-math.sty", "polyglossia.sty", "xltxtra.sty", "mathspec.sty"];

/** The stand-ins' texts, by name, read with `asset` (the platform's: the extension's shims/). */
export async function shimTexts(asset: (path: string) => Promise<Response>): Promise<Record<string, string>> {
  const out: Record<string, string> = {};
  for (const n of SHIMS) out[n] = await (await asset("shims/" + n)).text();
  return out;
}

/**
 * Whether a project that needs XeLaTeX can be approximated with pdfLaTeX and
 * the stand-ins: fonts substituted, Latin scripts only. Not CJK, Chinese
 * classes, or anything LuaTeX's (pdfTeX's fonts have no glyphs for them).
 */
export function approximable(main: string | undefined): boolean {
  if (needs(main) !== "xelatex" || !main) return false;
  const end = main.indexOf("\\begin{document}");
  const pre = (end < 0 ? main : main.slice(0, end)).replace(/(^|[^\\])%.*$/gm, "$1");
  return !/\\usepackage(\[[^\]]*\])?\{[^}]*\b(xeCJK|ctex|xepersian|bidi|arabxetex|xgreek|xunicode)\b|\\documentclass(\[[^\]]*\])?\{ctex/.test(pre);
}
