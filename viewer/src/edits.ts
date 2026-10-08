// Edits as CodeMirror gives them (UTF-16 offsets) and as PhiTeX takes them
// (UTF-8 byte offsets), and the batching of rapid ones. Pure: no DOM.

/** A replacement in UTF-16 code units, in the text as it is just before it. */
export interface Edit {
  from: number;
  to: number;
  text: string;
}

/** The same, in UTF-8 bytes. */
export interface ByteEdit {
  start: number;
  end: number;
  text: string;
}

/** UTF-8 length of `s.slice(from, to)` (lone surrogates count as U+FFFD: 3). */
export function utf8Len(s: string, from = 0, to = s.length): number {
  let n = 0;
  for (let i = from; i < to; i++) {
    const c = s.charCodeAt(i);
    if (c < 0x80) n += 1;
    else if (c < 0x800) n += 2;
    else if (c >= 0xd800 && c <= 0xdbff && i + 1 < to) {
      const d = s.charCodeAt(i + 1);
      if (d >= 0xdc00 && d <= 0xdfff) {
        n += 4;
        i++;
      } else n += 3;
    } else n += 3;
  }
  return n;
}

/**
 * The byte offset of UTF-16 offset `pos` in `s`. A `pos` inside a surrogate
 * pair (CodeMirror never gives one) is moved back to the pair's start, so
 * PhiTeX never sees a range off a character boundary.
 */
export function byteOffset(s: string, pos: number): number {
  if (pos > 0 && pos < s.length) {
    const c = s.charCodeAt(pos);
    const p = s.charCodeAt(pos - 1);
    if (c >= 0xdc00 && c <= 0xdfff && p >= 0xd800 && p <= 0xdbff) pos--;
  }
  return utf8Len(s, 0, pos);
}

export function apply(s: string, e: Edit): string {
  return s.slice(0, e.from) + e.text + s.slice(e.to);
}

/**
 * A CodeMirror ChangeSet (`iterChanges`: positions in the doc before the
 * transaction, applied at once) as edits applied one after another.
 */
export function sequential(
  iter: (f: (fromA: number, toA: number, fromB: number, toB: number, text: string) => void) => void,
): Edit[] {
  const out: Edit[] = [];
  // In ascending order, a change's place in the text after the ones before
  // it is its `fromB` (its place in the new doc).
  iter((fromA, toA, fromB, _toB, text) => out.push({ from: fromB, to: fromB + (toA - fromA), text }));
  return out;
}

/**
 * `b` after `a` as one edit, if they touch or overlap (typing, deleting,
 * autocompletion); else null. Order is kept: the result applied to the
 * text before `a` is `a` then `b`.
 */
export function merge(before: string, a: Edit, b: Edit): Edit | null {
  const aEnd = a.from + a.text.length; // (a's text, in the text after a)
  if (b.to < a.from || b.from > aEnd) return null;
  const from = Math.min(a.from, b.from);
  const endAfterA = Math.max(aEnd, b.to);
  // (what follows a's text, in the text before a, is shifted by a's delta)
  const to = endAfterA <= aEnd ? a.to : endAfterA - (a.text.length - (a.to - a.from));
  const afterA = apply(before.slice(from, to), { from: a.from - from, to: a.to - from, text: a.text });
  const text = apply(afterA, { from: b.from - from, to: b.to - from, text: b.text });
  return { from, to, text };
}

/** Edits of one file, queued between flushes, merged as they come. */
export class Batch {
  private edits: Edit[] = [];
  /** The text before the first queued edit. */
  private base: string;
  /** The text after all of them. */
  text: string;

  constructor(text: string) {
    this.base = text;
    this.text = text;
  }

  get size(): number {
    return this.edits.length;
  }

  push(e: Edit): void {
    const last = this.edits.at(-1);
    if (last) {
      // (the text before `last`: undo it on the current text is not
      // possible without it, so keep the text before each queued edit)
      const before = this.befores.at(-1)!;
      const m = merge(before, last, e);
      if (m) {
        this.edits[this.edits.length - 1] = m;
        this.text = apply(this.text, e);
        return;
      }
    }
    this.befores.push(this.text);
    this.edits.push(e);
    this.text = apply(this.text, e);
  }

  private befores: string[] = [];

  /** The queued edits in bytes, in order, each against the text before it. */
  take(): ByteEdit[] {
    const out: ByteEdit[] = [];
    let s = this.base;
    for (const e of this.edits) {
      const start = byteOffset(s, e.from);
      out.push({ start, end: start + utf8Len(s, e.from, e.to), text: e.text });
      s = apply(s, e);
    }
    this.edits = [];
    this.befores = [];
    this.base = this.text;
    return out;
  }
}

/** The UTF-16 offset of byte offset `byte` in `s` (its inverse of `byteOffset`). */
export function charOffset(s: string, byte: number): number {
  let n = 0;
  for (let i = 0; i < s.length; i++) {
    if (n >= byte) return i;
    const c = s.charCodeAt(i);
    if (c < 0x80) n += 1;
    else if (c < 0x800) n += 2;
    else if (c >= 0xd800 && c <= 0xdbff && i + 1 < s.length) {
      n += 4;
      i++;
    } else n += 3;
  }
  return s.length;
}
