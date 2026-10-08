// A preview drawn somewhere else than the session runs: every call the
// session makes on its sink, sent on as a message, and applied to a sink
// there. Overleaf's detached PDF tab (overleaf/src/mirror.ts, over a
// BroadcastChannel) and VS Code's webview (the session in the extension
// host, the panel in the webview) both take it this way.

import type { PreviewSink } from "./session.ts";

/** A sink call, sent on. */
export type Told = { t: "sink"; m: keyof PreviewSink; a: unknown[] } | { t: "up" };

/** `sink`, with every call also sent on `ch` (while `live()`). */
export function tee(sink: PreviewSink, ch: { postMessage(m: Told): void }, live: () => boolean): PreviewSink {
  return new Proxy(sink, {
    get(t, m: keyof PreviewSink) {
      const f = t[m] as ((...a: unknown[]) => void) | undefined;
      if (typeof f !== "function") return f;
      return (...a: unknown[]) => {
        if (live()) {
          try {
            ch.postMessage({ t: "sink", m, a } satisfies Told);
          } catch {
            /* (a value that doesn't clone: the session's side still has it) */
          }
        }
        return f.apply(t, a);
      };
    },
  });
}

/** A call `tee` sent, applied to `sink` (false: not a sink call). */
export function apply(sink: PreviewSink, told: Told): boolean {
  if (told?.t !== "sink") return false;
  const f = sink[told.m] as ((...a: unknown[]) => void) | undefined;
  f?.apply(sink, told.a);
  return true;
}
