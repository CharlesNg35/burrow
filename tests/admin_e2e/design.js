(() => {
  const out = [];
  const vw = document.documentElement.clientWidth;
  const vis = el => {
    const r = el.getBoundingClientRect();
    if (r.width < 2 || r.height < 2 || el.closest('.sr-only,[hidden],[aria-hidden=true]')) return false;
    const s = getComputedStyle(el);
    return s.visibility !== 'hidden' && s.display !== 'none' && parseFloat(s.opacity) > 0;
  };
  const words = el => (el.innerText || el.textContent || '').trim().replace(/\s+/g, ' ');
  const name = el => {
    const p = [];
    for (let e = el; e && e !== document.body && p.length < 3; e = e.parentElement) {
      const id = e.getAttribute('data-testid');
      p.unshift(e.tagName.toLowerCase() + (id ? '[' + id + ']' : ''));
    }
    return p.join('>');
  };
  const add = (rule, el, detail) => out.push(`${rule} ${name(el)}: ${String(detail).slice(0, 160)}`);
  const sentences = s => (s.match(/[.!?]\s+[A-Z]/g) || []).length + 1;
  const textRects = el => {
    const rects = [];
    const w = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    for (let n = w.nextNode(); n; n = w.nextNode()) {
      if (!n.textContent.trim() || !n.parentElement || !vis(n.parentElement)) continue;
      const g = document.createRange();
      g.selectNodeContents(n);
      for (const q of g.getClientRects()) if (q.width > 0.5 && q.height > 0.5) rects.push(q);
    }
    return rects;
  };
  const firstLine = el => {
    if (el.matches('button,input,select,textarea,svg,img,[role=switch],[role=radio],[role=checkbox]')) return el.getBoundingClientRect();
    const t = textRects(el);
    return t.length ? t[0] : el.getBoundingClientRect();
  };
  const sameLine = (a, b) => Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) > Math.min(a.height, b.height) / 2;
  const centre = r => (r.top + r.bottom) / 2;

  for (const p of document.querySelectorAll('[data-header-line]')) {
    if (!vis(p)) continue;
    const t = words(p);
    const lh = parseFloat(getComputedStyle(p).lineHeight) || 24;
    const lines = Math.round(p.getBoundingClientRect().height / lh);
    if (t.length > 80) add('R2 header line over 80 characters', p, `${t.length}: ${t}`);
    if (sentences(t) > 1) add('R3 header line holds more than one sentence', p, t);
    if (vw < 500 && lines > 2) add('R2 header line over 2 lines on a phone', p, `${lines} lines: ${t}`);
  }
  for (const s of document.querySelectorAll('[data-card-sub]')) {
    if (!vis(s)) continue;
    const t = words(s);
    if (t.length > 50) add('R2 card subline over 50 characters', s, `${t.length}: ${t}`);
  }
  for (const t of document.querySelectorAll('[data-testid=toast]')) {
    if (!vis(t)) continue;
    const s = words(t);
    if (s.length > 80) add('R2 toast over 80 characters', t, `${s.length}: ${s}`);
    const key = s.toLowerCase().replace(/[^a-z0-9]+/g, ' ').trim();
    if (key.length < 12) continue;
    for (const o of document.querySelectorAll('[data-header-line],[data-card-sub]')) {
      if (vis(o) && words(o).toLowerCase().replace(/[^a-z0-9]+/g, ' ').includes(key)) add('R1 the toast repeats a line of the page', o, s);
    }
  }
  const internal = /\b(endpoint|payload|schema|struct)\b|\b[a-z]+(_[a-z]+)*\[\d+\]/;
  const prose = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let n = prose.nextNode(); n; n = prose.nextNode()) {
    const el = n.parentElement;
    if (!el || el.closest('code,pre,.font-mono,input,textarea,script,style,[data-testid=about-page],[data-details]') || !vis(el)) continue;
    const m = n.textContent.match(internal);
    if (m) add('R5 internal word "' + m[0] + '" in plain text', el, n.textContent.trim().slice(0, 100));
  }
  const calm = [...document.querySelectorAll('[data-calm]')].filter(vis);
  if (calm.length) {
    const loud = [...document.querySelectorAll('[data-tone=danger],[data-tone=warning]')].filter(vis).filter(e => !e.closest('[data-testid=toasts],[role=dialog]'));
    if (loud.length) add('R4 a calm summary next to a problem', calm[0], `"${words(calm[0])}" with "${words(loud[0])}"`);
  }

  const ctx = document.createElement('canvas').getContext('2d');
  const measure = (f, ch) => { ctx.font = '20px ' + f; const m = ctx.measureText(ch); return [m.width, m.actualBoundingBoxLeft, m.actualBoundingBoxRight, m.actualBoundingBoxAscent, m.actualBoundingBoxDescent].map(x => x.toFixed(2)).join(','); };
  const glyph = (fam, ch) => {
    const a = measure(fam + ', serif', ch), b = measure(fam + ', monospace', ch);
    if (a !== b) return 'fallback';
    if (a === measure(fam + ', serif', '\u{10FFFD}')) return 'tofu';
    if (a === measure('serif', ch) && b === measure('monospace', ch)) return 'fallback';
    return 'ok';
  };
  const seen = new Map();
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    const el = n.parentElement;
    const t = n.textContent;
    if (!el || !t.trim() || el.closest('script,style,svg') || !vis(el)) continue;
    const fam = getComputedStyle(el).fontFamily.split(',')[0].trim();
    const loaded = [...document.fonts].some(f => f.family.replace(/"/g, '') === fam.replace(/"/g, '') && f.status === 'loaded');
    for (const ch of t) {
      const c = ch.codePointAt(0);
      if (!loaded || c < 0x2000 || /\s|\u200b/.test(ch) || '→←'.includes(ch)) continue;
      const k = fam + ch;
      if (!seen.has(k)) seen.set(k, glyph(fam, ch));
      const g = seen.get(k);
      if (g !== 'ok') { add('R9 a typed symbol the font lacks (' + g + '), use an Icon', el, `U+${c.toString(16).toUpperCase()} "${ch}" in "${t.trim().slice(0, 60)}"`); break; }
    }
    let i = -1;
    while ((i = t.indexOf('·', i + 1)) >= 0) {
      const g = document.createRange();
      g.setStart(n, i);
      g.setEnd(n, i + 1);
      const r = g.getBoundingClientRect();
      if (r.width < 1) continue;
      let box = el;
      while (box.parentElement && /^(inline|contents)$/.test(getComputedStyle(box).display)) box = box.parentElement;
      if (/flex|grid/.test(getComputedStyle(box.parentElement || box).display) && box.parentElement && box.parentElement !== document.body) box = box.parentElement;
      const part = (a, b) => { const q = document.createRange(); q.setStart(n, a); q.setEnd(n, b); return /\S/.test(q.toString()) ? [...q.getClientRects()] : []; };
      const others = [];
      const w2 = document.createTreeWalker(box, NodeFilter.SHOW_TEXT);
      for (let m = w2.nextNode(); m; m = w2.nextNode()) {
        if (m === n || !m.textContent.trim() || !m.parentElement || !vis(m.parentElement)) continue;
        const q = document.createRange(); q.selectNodeContents(m); others.push(...q.getClientRects());
      }
      const near = [...part(0, i), ...part(i + 1, t.length), ...others, ...[...box.querySelectorAll('svg,img,button,input')].filter(vis).map(e => e.getBoundingClientRect())]
        .filter(q => q.width > 0.5 && sameLine(q, r));
      const left = near.some(q => q.right <= r.left + 1 && q.right > r.left - 40);
      const right = near.some(q => q.left >= r.right - 1 && q.left < r.right + 40);
      if (!left || !right) add('R8 a separator ' + (!left ? 'starts' : 'ends') + ' a line', el, words(box).slice(0, 100));
    }
  }

  for (const row of document.querySelectorAll('body *')) {
    const cs = getComputedStyle(row);
    if (!/flex/.test(cs.display) || !cs.flexDirection.startsWith('row') || !vis(row) || row.closest('svg,[data-testid=toasts]')) continue;
    const items = [...row.children].filter(vis).filter(c => !/absolute|fixed/.test(getComputedStyle(c).position));
    if (items.length < 2 || items.some(c => c.getBoundingClientRect().height > 64)) continue;
    const ctl = c => c.matches('button,a[href].inline-flex,input,select,[role=switch]') || (c.children.length === 1 && c.children[0].matches('button,input,select,[role=switch]') && !words(c).replace(words(c.children[0]), '').trim());
    const ctls = items.filter(ctl), texts = items.filter(c => !ctl(c) && words(c).length > 1);
    for (const a of ctls) for (const b of texts) {
      const ra = a.getBoundingClientRect(), rb = firstLine(b), whole = b.getBoundingClientRect();
      if (!sameLine(ra, rb)) continue;
      const d = Math.min(Math.abs(centre(ra) - centre(rb)), Math.abs(centre(ra) - centre(whole)));
      if (d > 2) { add('R8 text and a control off-centre by ' + d.toFixed(1) + 'px', row, `"${words(a).slice(0, 30)}" / "${words(b).slice(0, 40)}"`); break; }
    }
  }
  const middle = cell => {
    const w = document.createTreeWalker(cell, NodeFilter.SHOW_TEXT);
    for (let n = w.nextNode(); n; n = w.nextNode()) {
      if (!n.textContent.trim() || !n.parentElement || !vis(n.parentElement)) continue;
      const g = document.createRange();
      g.selectNodeContents(n);
      const q = [...g.getClientRects()].find(r => r.width > 0.5 && r.height > 0.5);
      if (!q) continue;
      const f = getComputedStyle(n.parentElement);
      ctx.font = `${f.fontStyle} ${f.fontWeight} ${f.fontSize} ${f.fontFamily}`;
      const m = ctx.measureText(n.textContent.trim()), x = ctx.measureText('x');
      return q.top + m.fontBoundingBoxAscent - x.actualBoundingBoxAscent / 2;
    }
    return null;
  };
  for (const tr of document.querySelectorAll('tbody tr,[data-table-row]')) {
    if (!vis(tr)) continue;
    const cells = [...tr.children].filter(vis).map(td => [middle(td), words(td).slice(0, 14)]).filter(c => c[0] !== null);
    if (cells.length < 2) continue;
    const top = Math.min(...cells.map(c => c[0]));
    const first = Math.max(...cells.map(c => c[0])) - top;
    if (first > 2) add('R8 table cells do not line up on their first line, ' + first.toFixed(1) + 'px apart', tr, cells.map(c => c[1] + ' +' + (c[0] - top).toFixed(1)).join(' | '));
  }
  return [...new Set(out)].slice(0, 40);
})()
