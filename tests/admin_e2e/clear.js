(async () => {
  const frames = () => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)));
  const rects = s => [...document.querySelectorAll(s)].map(e => e.getBoundingClientRect()).filter(r => r.width > 1 && r.height > 1);
  const STACK = '[data-testid=toast],[data-sticky-bar]';
  if (!rects(STACK).length || rects('[role=dialog]').length) return [];
  const y = scrollY;
  scrollTo(0, document.documentElement.scrollHeight);
  await frames();
  await new Promise(r => setTimeout(r, 300));
  const stack = rects(STACK);
  const seen = el => {
    const r = el.getBoundingClientRect();
    let { top, bottom, left, right } = r;
    for (let p = el.parentElement; p && p !== document.body; p = p.parentElement) {
      if (getComputedStyle(p).overflow === 'visible') continue;
      const q = p.getBoundingClientRect();
      top = Math.max(top, q.top); bottom = Math.min(bottom, q.bottom);
      left = Math.max(left, q.left); right = Math.min(right, q.right);
    }
    return { top, bottom, left, right };
  };
  const out = [];
  for (const c of document.querySelectorAll('main .rounded-card')) {
    if (c.closest('[data-sticky-bar]') || c.querySelector(STACK)) continue;
    const r = seen(c);
    if (r.bottom - r.top < 2 || r.right - r.left < 2) continue;
    for (const s of stack) {
      if (r.left < s.right && s.left < r.right && r.top < s.bottom && s.top < r.bottom - 1) {
        const what = (c.getAttribute('data-testid') || (c.innerText || '').trim().split('\n')[0]).slice(0, 60);
        out.push(`"${what}" ends at ${r.bottom.toFixed(1)}, under a toast or the save bar from ${s.top.toFixed(1)} (scrolled to the end, ${innerWidth} px wide)`);
        break;
      }
    }
  }
  scrollTo(0, y);
  await frames();
  return out;
})()
