// What the viewer says to a person: a core's error in plain words.

/**
 * A core's error as a person reads it: the hosts' "core trapped (…):" tag
 * (what the session tells a lost core by) dropped; a host's own sentence
 * kept, an engine's raw error said as a crash.
 */
export function plainWhy(e: string): string {
  const m = /^core trapped \([^)]*\):\s*([\s\S]*)$/.exec(e);
  if (!m) return sentence(e);
  const t = m[1].trim();
  return /[.!?]$/.test(t) ? t : `The engine crashed (${t || "no reason given"}).`;
}

/** `s` ended as a sentence (a full stop added if it has none; its first letter as written: "phitex watch" is a command's name). */
export function sentence(s: string): string {
  const t = s.trim();
  return !t || /[.!?…]$/.test(t) ? t : `${t}.`;
}
