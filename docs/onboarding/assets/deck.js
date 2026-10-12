/* Deck behaviour (trimmed from the CS394A week-7 deck): code blocks with one
   span per line, fragment-driven line highlights (data-hl), step classes
   s1..s9 on the current slide, a table of contents on T. */
(() => {
  if (/static/.test(location.search)) document.documentElement.classList.add('static');
  const $$ = (s, r = document) => [...r.querySelectorAll(s)];
  const hljs = RevealHighlight().hljs;
  $$('pre.code').forEach(pre => {
    const lines = pre.textContent.replace(/^\n+/, '').replace(/\s+$/, '').split('\n');
    const ind = Math.min(...lines.filter(l => l.trim()).map(l => l.match(/^ */)[0].length));
    let lang = pre.dataset.lang || 'rust';
    if (!hljs.getLanguage(lang)) lang = 'plaintext';
    pre.innerHTML = lines.map(l => l.slice(ind)).map(l => `<span class="ln">${hljs.highlight(l, { language: lang }).value || ' '}</span>`).join('');
    pre.classList.add('hljs');
    if (pre.dataset.start) pre.style.counterReset = `ln ${+pre.dataset.start - 1}`;
  });
  function parseList(r) {
    const set = new Set();
    r.split(',').filter(Boolean).forEach(p => { const [a, b = a] = p.split('-').map(Number); for (let i = a; i <= b; i++) set.add(i); });
    return set;
  }
  function applyHL(slide) {
    $$('pre.code', slide).forEach((pre, ix) => {
      let spec = pre.dataset.hl || null, best = -1;
      $$('.fragment.visible[data-hl]', slide).forEach(f => {
        if (ix !== 0) return;
        const i = +f.dataset.fragmentIndex; if (i >= best) { best = i; spec = f.dataset.hl; }
      });
      const lns = $$('.ln', pre);
      lns.forEach(l => l.classList.remove('on', 'amber', 'teal', 'blue', 'rose'));
      pre.classList.toggle('has-hl', !!spec);
      if (!spec) return;
      spec.split(';').filter(Boolean).forEach(g => { const [r, cls = 'on'] = g.split(':'); parseList(r).forEach(n => lns[n - 1] && lns[n - 1].classList.add(cls)); });
    });
  }
  function sync(slide) {
    if (!slide) return;
    const n = new Set($$('.fragment.visible', slide).map(f => f.dataset.fragmentIndex)).size;
    for (let i = 1; i <= 9; i++) slide.classList.toggle('s' + i, i <= n);
    applyHL(slide);
  }
  function measure(slide) { $$('.draw', slide).forEach(p => { try { p.style.setProperty('--len', Math.ceil(p.getTotalLength()) + 1); } catch (e) { /* not rendered */ } }); }
  Reveal.initialize({
    hash: true, width: 1280, height: 720, margin: 0.03, center: false,
    controls: true, controlsTutorial: false, progress: true, slideNumber: 'c/t',
    transition: 'fade', backgroundTransition: 'fade', pdfSeparateFragments: false,
    plugins: [RevealNotes]
  }).then(() => { $$('.reveal .slides section').forEach(s => { measure(s); applyHL(s); }); sync(Reveal.getCurrentSlide()); });
  Reveal.on('slidechanged', e => { measure(e.currentSlide); sync(e.currentSlide); });
  Reveal.on('fragmentshown', () => sync(Reveal.getCurrentSlide()));
  Reveal.on('fragmenthidden', () => sync(Reveal.getCurrentSlide()));

  const toc = document.createElement('div'); toc.className = 'toc'; document.body.appendChild(toc);
  const inner = document.createElement('div'); inner.className = 'toc-in'; toc.appendChild(inner);
  inner.innerHTML = '<div class="toc-h">Contents · press T or Esc to close</div>';
  const cols = document.createElement('div'); cols.className = 'toc-cols'; inner.appendChild(cols);
  let grp = null;
  Reveal.getSlides().forEach((sl, i) => {
    const h = sl.querySelector('h1, h2'), k = sl.querySelector('.kicker');
    const title = (h ? h.textContent : '').replace(/\s+/g, ' ').trim();
    const part = k ? k.textContent.split('·')[0].trim() : '';
    if (!grp || part !== grp.dataset.part) {
      grp = document.createElement('div'); grp.className = 'toc-g'; grp.dataset.part = part; grp.style.setProperty('--c', 'var(--p2)'); cols.appendChild(grp);
      const t = document.createElement('div'); t.className = 'toc-gt'; t.textContent = part; grp.appendChild(t);
    }
    const a = document.createElement('a'); a.className = 'toc-a'; a.href = '#'; a.innerHTML = `<b>${i + 1}</b> `; a.append(title); grp.appendChild(a);
    a.addEventListener('click', e => { e.preventDefault(); toc.classList.remove('on'); Reveal.slide(i); });
  });
  Reveal.addKeyBinding({ keyCode: 84, key: 'T', description: 'Table of contents' }, () => toc.classList.toggle('on'));
  document.addEventListener('keydown', e => { if (e.key === 'Escape' && toc.classList.contains('on')) { e.stopPropagation(); toc.classList.remove('on'); } }, true);
})();
