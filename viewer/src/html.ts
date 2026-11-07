// Markup without strings parsed as HTML: the stores' linters (Firefox's
// addons-linter) warn at every innerHTML or insertAdjacentHTML given a
// string that is not a literal, the viewer's in the extensions too. html``
// parses only a template's literal parts, which the code writes, and puts
// each value in through the DOM; rich() is text with a few inline tags.

/** What a value in an `html` template may be: text (or a number), a node, a list of them, or nothing (null, undefined, false). */
export type Slot = string | number | boolean | Node | null | undefined | readonly Slot[];

const MARK = "phx-slot-";
/** Each template's markup, parsed once (its literal parts only, the values' places marked). */
const parsed = new WeakMap<TemplateStringsArray, DocumentFragment>();
const parsedSvg = new WeakMap<TemplateStringsArray, DocumentFragment>();

/**
 * Markup from a template's literal parts, which only the code writes: they
 * are parsed (in a <template>, so rows and list items parse anywhere), and
 * each value is put in its place through the DOM, never parsed: text as a
 * text node, a node as itself, a list as its items; in a quoted attribute
 * (`class="a ${x}"`), as the attribute's text, or, the whole of it and true
 * or false (or nothing), the attribute there or not (`disabled="${busy}"`). No value may be an event
 * handler (`on…`). The stores' linters flag HTML assigned from strings; this
 * is the viewer's one way to write markup.
 */
export function html(strings: TemplateStringsArray, ...values: Slot[]): DocumentFragment {
  return build(strings, values, false);
}

/** As html``, its markup SVG's (parsed in an <svg>: <path>, <circle>, <text> made in SVG's namespace, to go in one). */
export function svg(strings: TemplateStringsArray, ...values: Slot[]): DocumentFragment {
  return build(strings, values, true);
}

function build(strings: TemplateStringsArray, values: Slot[], inSvg: boolean): DocumentFragment {
  const cache = inSvg ? parsedSvg : parsed;
  let base = cache.get(strings);
  if (!base) {
    let src = "";
    strings.forEach((s, i) => {
      src += s;
      if (i === values.length) return;
      // (inside a tag, after an attribute's opening quote: the attribute's text; else a node's place)
      // (raw text: a value inside <style>, <textarea>, <title> or <script> would be its marker's text)
      if (/<(style|textarea|title|script)\b[^>]*>(?![\s\S]*<\/\1>)[\s\S]*$/i.test(src)) throw new Error("html: no value inside <style>, <textarea>, <title> or <script>");
      if (/<[a-zA-Z][^<>]*=\s*"[^"]*$/.test(src)) src += `${MARK}${i}`;
      else if (/<[a-zA-Z][^<>]*$/.test(src)) throw new Error(`html: a value inside a tag must be in a quoted attribute (after ${JSON.stringify(s.slice(-40))})`);
      else src += `<template ${MARK}${i}></template>`;
    });
    const t = new DOMParser().parseFromString(`<template>${inSvg ? `<svg>${src}</svg>` : src}</template>`, "text/html").querySelector("template")!.content;
    if (inSvg) {
      // (kept in the parser's inert document, as a template's content is)
      base = t.ownerDocument.createDocumentFragment();
      base.append(...t.firstElementChild!.childNodes);
    } else base = t;
    // (the template's own line breaks between elements are its layout, not text: dropped, as JSX does)
    const drop = (n: Node) => {
      for (const k of [...n.childNodes]) {
        if (k.nodeType === Node.TEXT_NODE && /^\s*$/.test(k.textContent!) && k.textContent!.includes("\n")) k.remove();
        else drop(k instanceof HTMLTemplateElement ? k.content : k);
      }
    };
    drop(base);
    cache.set(strings, base);
  }
  // (filled in the parser's own, inert document, then adopted where it is put: no attribute is ever a marker in the page)
  const frag = base.cloneNode(true) as DocumentFragment;
  const nodes = (v: Slot): (Node | string)[] => (v === null || v === undefined || v === false ? [] : Array.isArray(v) ? v.flatMap(nodes) : [v instanceof Node ? v : String(v)]);
  const walk = (root: ParentNode) => {
    for (const el of [...root.querySelectorAll("*")]) {
      if (el.localName === "template" && el.attributes.length === 1 && el.attributes[0].name.startsWith(MARK)) {
        el.replaceWith(...nodes(values[+el.attributes[0].name.slice(MARK.length)]));
        continue;
      }
      for (const a of [...el.attributes]) {
        if (!a.value.includes(MARK)) continue;
        if (a.name.startsWith("on")) throw new Error(`html: no value in an event handler (${a.name})`);
        // (a value that is the whole attribute, true or false: the attribute there or not, disabled="${!ok}";
        // an aria- or data- one says "true" or "false")
        const whole = new RegExp(`^${MARK}(\\d+)$`).exec(a.value), v = whole && values[+whole[1]];
        if (whole && !/^(aria|data)-/.test(a.name) && (v === true || v === false || v === null || v === undefined)) {
          if (v === true) el.setAttribute(a.name, "");
          else el.removeAttribute(a.name);
          continue;
        }
        el.setAttribute(a.name, a.value.replace(new RegExp(`${MARK}(\\d+)`, "g"), (_, n) => nodes(values[+n]).map((x) => (typeof x === "string" ? x : x.textContent)).join("")));
      }
      if (el instanceof HTMLTemplateElement) walk(el.content);
    }
  };
  walk(frag);
  return frag;
}

/** Text with a few inline tags in it (<b>, <code>, <i>, <kbd>; the news' items): those as elements, the rest as text. */
export function rich(text: string): DocumentFragment {
  const f = document.createDocumentFragment();
  const open: Node[] = [f];
  for (const part of text.split(/(<\/?(?:b|code|i|kbd)>)/)) {
    const tag = /^<(\/?)(b|code|i|kbd)>$/.exec(part);
    if (!tag) part && open[open.length - 1].appendChild(document.createTextNode(part));
    else if (!tag[1]) open.push(open[open.length - 1].appendChild(document.createElement(tag[2])));
    else if (open.length > 1) open.pop();
  }
  return f;
}

/**
 * Art the build made (opening-art.ts: SVG from the logo files, by
 * viewer/tools/opening-art.mjs), as SVG nodes: never data from outside the
 * code. Parsed as XML, which makes inert nodes; a script or an event
 * handler in it refused all the same.
 */
export function svgArt(markup: string): DocumentFragment {
  if (/<script|\son[a-z]+\s*=/i.test(markup)) throw new Error("svgArt: art has no script or event handler");
  const doc = new DOMParser().parseFromString(`<svg xmlns="http://www.w3.org/2000/svg">${markup}</svg>`, "image/svg+xml");
  if (doc.querySelector("parsererror")) throw new Error("svgArt: not well-formed SVG");
  const f = document.createDocumentFragment();
  f.append(...[...doc.documentElement.childNodes].map((n) => document.importNode(n, true)));
  return f;
}
