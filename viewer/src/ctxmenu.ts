// A small menu at the pointer (a right-click on a mark of the pages): its
// items as buttons, the first focused, ↑ ↓ Home End to move, Enter or a
// click to choose, Esc, a press outside it or a scroll to close. One at a
// time; nothing of it runs till it is opened.

import { html } from "./html.ts";

export const MENU_CSS = `
.phx-ctx { position: fixed; z-index: 32; min-width: 170px; padding: 4px; background: var(--ph-menu, var(--ph-bar)); color: var(--ph-fg); border: 1px solid var(--ph-border); border-radius: 8px; box-shadow: 0 8px 24px rgb(15 25 40 / 22%); font: 13px/1.4 var(--ph-font, "Noto Sans", system-ui, sans-serif); text-align: left; }
.phx-ctx button { display: flex; gap: 8px; align-items: center; width: 100%; border: 0; background: transparent; color: inherit; font: inherit; text-align: left; padding: 5px 10px; border-radius: 5px; cursor: pointer; white-space: nowrap; }
.phx-ctx button:hover, .phx-ctx button:focus-visible { background: var(--ph-hover); outline: none; }
.phx-ctx button.accept { color: var(--ph-success, #1a7f37); }
.phx-ctx button.reject { color: var(--ph-danger, #c23a2f); }
.phx-ctx hr { border: 0; border-top: 1px solid var(--ph-border); margin: 4px 2px; }
`;

export interface MenuItem {
  label: string;
  /** Chosen: done, the menu closed first. */
  act: () => void;
  cls?: string;
  /** A rule above it. */
  sep?: boolean;
}

let open: { el: HTMLElement; close: () => void } | null = null;

/** The menu open now closed (another opens, the page it is on goes). */
export function closeMenu(): void {
  open?.close();
}

/** Whether a menu is open. */
export function menuOpen(): boolean {
  return !!open;
}

/** A menu of `items` at (x, y) (the viewport's), in `host` (the panel's window: its tokens colour it). */
export function contextMenu(host: HTMLElement, x: number, y: number, items: MenuItem[], label = "Actions"): void {
  closeMenu();
  const el = html`<div class="phx-ctx" role="menu" aria-label="${label}">${items.map((it, i) => html`${it.sep ? html`<hr>` : ""}<button type="button" role="menuitem" class="${it.cls ?? null}" data-i="${String(i)}" tabindex="-1">${it.label}</button>`)}</div>`.firstElementChild as HTMLElement;
  host.append(el);
  const was = (host.getRootNode() as ShadowRoot | Document).activeElement as HTMLElement | null;
  const buttons = [...el.querySelectorAll<HTMLButtonElement>("button")];
  // (placed at the pointer, inside the window's view; then moved by where it landed: a fixed box is placed in its
  // containing block, which a host's transform makes its pane)
  const w = el.offsetWidth, h = el.offsetHeight;
  const left = Math.max(4, Math.min(innerWidth - w - 4, x)), top = Math.max(4, Math.min(innerHeight - h - 4, y));
  el.style.left = `${left}px`;
  el.style.top = `${top}px`;
  const at = el.getBoundingClientRect();
  if (Math.abs(at.left - left) > 0.5 || Math.abs(at.top - top) > 0.5) (el.style.left = `${2 * left - at.left}px`), (el.style.top = `${2 * top - at.top}px`);
  const close = (refocus = false) => {
    if (open?.el !== el) return;
    open = null;
    el.remove();
    document.removeEventListener("pointerdown", outside, true);
    removeEventListener("scroll", gone, true);
    if (refocus) was?.focus?.({ preventScroll: true });
  };
  const outside = (e: Event) => !e.composedPath().includes(el) && close();
  const gone = (e: Event) => !e.composedPath().includes(el) && close();
  document.addEventListener("pointerdown", outside, true);
  addEventListener("scroll", gone, true);
  el.addEventListener("click", (e) => {
    const b = (e.target as HTMLElement).closest<HTMLButtonElement>("button[data-i]");
    if (!b) return;
    e.stopPropagation();
    close();
    items[Number(b.dataset.i)].act();
  });
  el.addEventListener("keydown", (e) => {
    const i = buttons.indexOf(e.target as HTMLButtonElement);
    const to = (j: number) => buttons[(j + buttons.length) % buttons.length].focus();
    if (e.key === "ArrowDown") to(i + 1);
    else if (e.key === "ArrowUp") to(i - 1);
    else if (e.key === "Home") to(0);
    else if (e.key === "End") to(buttons.length - 1);
    // (Esc: the menu alone, the topmost thing open; the focus back where it was)
    else if (e.key === "Escape" || e.key === "Tab") close(true);
    else return;
    e.preventDefault();
    e.stopPropagation();
  });
  el.addEventListener("contextmenu", (e) => e.preventDefault());
  open = { el, close: () => close() };
  buttons[0]?.focus({ preventScroll: true });
}
