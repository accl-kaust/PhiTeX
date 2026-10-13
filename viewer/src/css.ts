// The renderer's own CSS (the pages' slots), for a host to put in its
// document or shadow root. The host's chrome (bars, panels, dark mode, its
// stage's background) stays the host's.

import { PAGE_CSS } from "./print.ts";

export const VIEWER_CSS = `
.slot { position: relative; contain: layout; background: #fff; }
.slot canvas { display: block; width: 100%; height: 100%; margin: 0; }
${PAGE_CSS}`;
