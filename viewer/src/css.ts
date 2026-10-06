// The renderer's own CSS (the pages' slots, a settled page drawn as a
// picture with its live text over it: page2.ts's raster), for a host to put
// in its document or shadow root. The host's chrome (bars, panels, dark
// mode, its stage's background) stays the host's.

export const VIEWER_CSS = `
.slot { position: relative; contain: layout paint; }
/* (a page as a picture while it is unchanged, its live text over it; see page2.ts raster) */
.slot > img.ras { position: absolute; inset: 0; width: 100%; height: 100%; pointer-events: none; user-select: none; }
.slot > svg.page { position: relative; }
.slot > svg.page.ras g.c > :not(text) { display: none; }
.slot > svg.page.ras rect.paper { fill: none; }
.slot > svg.page.ras { background: transparent; }
.slot svg.page, .slot img { display: block; width: 100%; height: 100%; margin: 0; }
.slot svg.page text { white-space: pre; }
`;
