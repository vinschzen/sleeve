// Sleeve UI. No framework, no build step: the whole front end is three small files.
// Backends: 'tauri' (the app), 'dev' (sleeve-dev server in a browser), 'demo' (invented data).
const T = window.__TAURI__;
const MODE = T ? 'tauri' : /^(localhost|127\.0\.0\.1)$/.test(location.hostname) && location.port ? 'dev' : 'demo';
const DEMO = MODE === 'demo';

async function call(cmd, args = {}) {
  if (MODE === 'tauri') return T.core.invoke('call', { cmd, args });
  if (DEMO) return Demo.call(cmd, args);
  const r = await fetch('/api/' + cmd, { method: 'POST', body: JSON.stringify(args) });
  const j = await r.json();
  if (!r.ok) throw new Error(j.error);
  return j;
}

const $ = s => document.querySelector(s);
const esc = s => String(s ?? '').replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
const num = n => Number(n || 0).toLocaleString('en-US');
const two = n => String(n).padStart(2, '0');
const mmss = ms => { const s = Math.max(0, Math.round(ms / 1000)); return Math.floor(s / 60) + ':' + two(s % 60); };
const isDsd = t => t.codec === 'DSD' || t.codec === 'DST';
// DSD is named by its multiple of the CD rate: DSD64 = 64 x 44.1 kHz = 2.8224 MHz, 1 bit.
const spec = t => isDsd(t) ? t.codec + Math.round(t.rate / 44100) : t.bits ? `${t.codec} ${t.bits}/${+(t.rate / 1000).toFixed(1)}` : `${t.codec} ${t.kbps}`;
const TIER = ['Lossy', 'Lossy 256+', 'Lossless', 'Hi-res'];
const dots = n => `<span class="q" title="${TIER[n]}">${[1, 2, 3].map(i => `<i${i <= n ? ' class="on"' : ''}></i>`).join('')}</span>`;
const press = (group, el) => group.querySelectorAll('button').forEach(b => b.setAttribute('aria-pressed', b === el));
const store = { get: k => { try { return localStorage.getItem(k); } catch { return null; } }, set: (k, v) => { try { localStorage.setItem(k, v); } catch {} } };

let toastTimer;
function toast(msg, bad) {
  const el = $('#toast');
  el.textContent = msg; el.hidden = false; el.classList.toggle('bad', !!bad);
  clearTimeout(toastTimer); toastTimer = setTimeout(() => (el.hidden = true), 3200);
}
const fail = e => toast(String(e?.message || e), true);

/* ---------- Touch feedback ---------- */
// Phones: the Tauri haptics plugin (Taptic Engine on iOS, vibrator on Android). Browsers: the Vibration API where it exists.
const still = matchMedia('(prefers-reduced-motion: reduce)').matches;
let nativeHaptics = !!T;
function haptic(style = 'light') {
  try {
    if (nativeHaptics) T.core.invoke('plugin:haptics|impact_feedback', { style }).catch(() => { nativeHaptics = false; });
    else if (navigator.vibrate) navigator.vibrate({ light: 8, medium: 14, heavy: 26 }[style]);
  } catch {}
}
// Controls answer on touch-down; rows answer on tap, so scrolling a list never buzzes.
addEventListener('pointerdown', e => { const el = e.target.closest?.('.disc, button:not(.row):not(.top)'); if (el) haptic(el.matches('.disc, .btn') ? 'medium' : 'light'); }, { capture: true, passive: true });
addEventListener('click', e => { if (e.target.closest?.('.row, .top')) haptic('light'); }, { capture: true });
addEventListener('touchstart', () => {}, { passive: true }); // lets :active styles fire on iOS
const pop = el => { el.classList.remove('pop'); void el.offsetWidth; el.classList.add('pop'); };
function countTo(el, to, fmt) {
  const from = el._v || 0, t0 = performance.now();
  el._v = to;
  if (still) return void (el.textContent = fmt(to));
  (function f(now) { const k = Math.min(1, (now - t0) / 340); el.textContent = fmt(from + (to - from) * (1 - Math.pow(1 - k, 4))); if (k < 1) requestAnimationFrame(f); })(t0);
}

/* ---------- Virtual list: renders ~25 rows no matter how many the library holds ---------- */
function VList(box, rowH, render, onTap, onHold, onIdle) {
  const pad = document.createElement('div'), win = document.createElement('div'), empty = document.createElement('div');
  pad.className = 'vpad'; win.className = 'vwin'; empty.className = 'empty'; empty.hidden = true;
  pad.append(win); box.append(pad, empty);
  const PAGE = 100, MAXH = 8e6; // browsers cap element height, so very long lists scroll proportionally
  let total = 0, fetcher = null, pages = new Map(), gen = 0, a0 = -1, b0 = -1, raf = 0, idle = 0;
  function page(p) {
    if (pages.has(p)) return pages.get(p);
    pages.set(p, null);
    const g = gen;
    fetcher(p * PAGE, PAGE).then(r => {
      if (g !== gen) return;
      pages.set(p, r.rows);
      if (pages.size > 30) for (const k of pages.keys()) { if (Math.abs(k - p) > 3) { pages.delete(k); break; } }
      a0 = -1; draw();
    }, () => g === gen && pages.delete(p));
    return null;
  }
  const row = i => { const p = page(Math.floor(i / PAGE)); return p ? p[i % PAGE] : null; };
  function draw() {
    raf = 0;
    const vh = box.clientHeight, st = box.scrollTop, realH = pad.offsetHeight, virtH = total * rowH;
    const vTop = st * (virtH > realH && realH > vh ? (virtH - vh) / (realH - vh) : 1);
    const top = Math.floor(vTop / rowH), a = Math.max(0, top - 5), b = Math.min(total, top + Math.ceil(vh / rowH) + 6);
    win.style.transform = `translateY(${st - (vTop - a * rowH)}px)`;
    if (a === a0 && b === b0) return;
    a0 = a; b0 = b;
    let h = '';
    for (let i = a; i < b; i++) { const r = row(i); h += r ? render(r, i) : '<div class="row ghost-row"><span class="rt"><b>&middot;&middot;&middot;</b></span></div>'; }
    win.innerHTML = h;
    clearTimeout(idle); idle = setTimeout(onIdle, 110); // artwork loads once scrolling settles
  }
  // Press and hold a row for its menu; a hold never also counts as a tap.
  let hold = 0, held = false, hx = 0, hy = 0;
  const drop = () => { clearTimeout(hold); hold = 0; };
  box.addEventListener('pointerdown', e => {
    const el = e.target.closest('[data-i]');
    held = false; drop();
    if (!el || e.button) return;
    hx = e.clientX; hy = e.clientY;
    hold = setTimeout(() => { hold = 0; const r = row(+el.dataset.i); if (r && onHold(r, +el.dataset.i)) held = true; }, 460);
  });
  box.addEventListener('pointermove', e => { if (hold && Math.hypot(e.clientX - hx, e.clientY - hy) > 8) drop(); });
  for (const ev of ['pointerup', 'pointercancel', 'pointerleave']) box.addEventListener(ev, drop);
  box.addEventListener('contextmenu', e => { e.preventDefault(); const el = e.target.closest('[data-i]'), r = el && e.button === 2 && row(+el.dataset.i); if (r) onHold(r, +el.dataset.i); });
  box.addEventListener('scroll', () => { drop(); if (!raf) raf = requestAnimationFrame(draw); }, { passive: true });
  box.addEventListener('click', e => { if (held) return void (held = false); const el = e.target.closest('[data-i]'); const r = el && row(+el.dataset.i); if (r) onTap(r, +el.dataset.i); });
  new ResizeObserver(() => { a0 = -1; draw(); }).observe(box);
  return {
    get total() { return total; },
    redraw() { a0 = -1; draw(); },
    async reset(f, emptyText) {
      fetcher = f; pages = new Map(); a0 = -1; box.scrollTop = 0;
      const g = ++gen, r = await f(0, PAGE);
      if (g !== gen) return;
      total = r.total; pages.set(0, r.rows);
      pad.style.height = Math.min(total * rowH, MAXH) + 'px';
      empty.hidden = total > 0; empty.textContent = emptyText;
      draw();
    },
  };
}

/* ---------- Library ---------- */
const L = { tab: 'tracks', q: '', sort: 'title', drill: null, fetch: null };

/* Artwork. Real covers come from the core; albums without one get a generated sleeve. */
const BLANK = 'data:image/gif;base64,R0lGODlhAQABAAAAACH5BAEKAAEALAAAAAABAAEAAAICTAEAOw==';
const thumbs = new Map(); // album id -> thumbnail URL
const coverUrl = (r, id, thumb) => r.url || (!r.path ? null : MODE === 'tauri' ? T.core.convertFileSrc(r.path) : `/${thumb ? 'athumb' : 'acover'}/${id}?v=${encodeURIComponent(r.path.slice(-22))}`);
let sleeveCv;
function sleeveUrl(al, size) {
  sleeveCv ||= Object.assign(document.createElement('canvas'), { width: 600, height: 600 });
  art(sleeveCv, al);
  const out = Object.assign(document.createElement('canvas'), { width: size, height: size });
  out.getContext('2d').drawImage(sleeveCv, 0, 0, size, size);
  return out.toDataURL('image/jpeg', .8);
}
function hydrate() {
  for (const img of document.querySelectorAll('#list img[data-al]')) {
    const id = +img.dataset.al;
    if (thumbs.has(id)) continue;
    thumbs.set(id, img.src = sleeveUrl({ album_id: id, artist: img.dataset.ar, album: img.dataset.ti }, 132));
    call('cover', { album_id: id, thumb: true }).then(r => {
      const u = coverUrl(r, id, true);
      if (u) { thumbs.set(id, u); document.querySelectorAll(`#list img[data-al="${id}"]`).forEach(i => (i.src = u)); }
    }).catch(() => {});
  }
}
// Album page header, gathered from the album's tracks.
async function paintDrill() {
  const d = L.drill;
  $('#v-library').classList.toggle('drilled', !!d); $('#drill').hidden = !d;
  if (!d) return;
  const album = d.kind === 'album';
  $('#dh-kind').textContent = `(${d.kind})`; $('#dh-title').textContent = d.name; $('#dh-sub').textContent = ''; $('#dh-meta').innerHTML = '&nbsp;';
  $('#dh-cover').hidden = !album; document.querySelector('.dh').classList.toggle('plain', !album);
  $('#drill-edit').hidden = d.kind === 'artist';
  if (!album) return;
  const al = d.al = await call('album', { album_id: d.id });
  if (L.drill !== d) return;
  const mb = al.size >= 1e9 ? (al.size / 1e9).toFixed(2) + ' GB' : Math.round(al.size / 1e6) + ' MB';
  $('#dh-title').textContent = al.title;
  $('#dh-sub').textContent = al.artist + (al.year ? ' · ' + (al.year_min && al.year_min !== al.year ? al.year_min + '–' + al.year : al.year) : '');
  $('#dh-meta').textContent = [`${al.n} tracks`, `${Math.round(al.ms / 60000)} min`, mb, al.formats.map(spec).join(', '), al.genres].filter(Boolean).join(' · ');
  $('#dh-img').src = sleeveUrl({ album_id: al.id, artist: al.artist, album: al.title }, 360);
  call('cover', { album_id: al.id }).then(r => { const u = coverUrl(r, al.id); if (u && L.drill === d) $('#dh-img').src = u; }).catch(() => {});
}

const SORTS = { title: '(A–Z)', new: '(New)', quality: '(Best)' };
const rowHtml = (i, on, title, sub, right, lead = '') => `<button class="row${on ? ' on' : ''}" data-i="${i}">${lead}<span class="rt"><b>${esc(title)}</b><span>${esc(sub)}</span></span><span class="rm">${right}</span></button>`;
const trackRow = (t, i) => rowHtml(i, P.t && P.t.id === t.id, t.title, `${t.artist} — ${t.album}`, `${dots(t.tier)}<span>${spec(t)} &middot; ${mmss(t.ms)}</span>`);
const list = VList($('#list'), 60, (r, i) => L.mode === 'tracks' ? (L.drill?.kind === 'album' ? rowHtml(i, P.t && P.t.id === r.id, r.title, r.artist, `${dots(r.tier)}<span>${spec(r)} &middot; ${mmss(r.ms)}</span>`) : trackRow(r, i))
  : L.mode === 'albums' ? rowHtml(i, 0, r.title, `${r.artist}${r.year ? ' · ' + r.year : ''}`, `${dots(r.tier)}<span>${r.n} tracks</span>`,
    `<img class="thumb" alt="" data-al="${r.id}" data-ar="${esc(r.artist)}" data-ti="${esc(r.title)}" src="${thumbs.get(r.id) || BLANK}">`)
  : rowHtml(i, 0, r.name, `${num(r.n)} tracks`, ''),
(r, i) => {
  if (L.mode === 'tracks') return play(r, { fetch: L.fetch, total: list.total, i });
  L.drill = { kind: L.mode.slice(0, -1), id: r.id, name: r.title || r.name };
  refresh();
},
r => { if (L.mode !== 'tracks') return false; haptic('medium'); trackMenu(r); return true; },
hydrate);

function refresh() {
  const { tab, q, sort, drill } = L;
  L.mode = drill ? 'tracks' : tab;
  $('#sort').hidden = L.mode !== 'tracks' || !!q || !!drill;
  $('#sort').textContent = SORTS[sort];
  $('#pl-new').hidden = tab !== 'playlists' || !!drill;
  paintDrill();
  const base = drill ? { [drill.kind + '_id']: drill.id } : { q, sort };
  L.fetch = (offset, limit) => call(L.mode, { ...base, offset, limit });
  list.reset(L.fetch, drill?.kind === 'playlist' ? 'This playlist is empty. Press and hold any track, then choose Add to playlist.'
    : q ? `Nothing matches “${q}”.` : tab === 'playlists' ? 'No playlists yet. Tap (+ New) to make one.'
    : 'No music indexed yet. Tap (Scan folder) to add a folder.')
    .then(() => { if (drill && drill === L.drill && drill.kind !== 'album') $('#dh-meta').textContent = `${num(list.total)} tracks`; }).catch(fail);
}
async function loadSummary() {
  const s = await call('summary');
  const size = s.bytes > 1e12 ? (s.bytes / 1e12).toFixed(2) + ' TB' : (s.bytes / 1e9).toFixed(1) + ' GB';
  $('#lib-meta').textContent = `${num(s.tracks)} tracks · ${num(s.albums)} albums · ${size} · ${Math.round((s.tiers[2] + s.tiers[3]) / Math.max(1, s.tracks) * 100)}% lossless`;
}
$('#lib-tabs').onclick = e => { const b = e.target.closest('button'); if (!b) return; press($('#lib-tabs'), b); L.tab = b.dataset.tab; L.drill = null; refresh(); };
$('#sort').onclick = () => { const k = Object.keys(SORTS); L.sort = k[(k.indexOf(L.sort) + 1) % k.length]; refresh(); };
$('#crumb').onclick = () => { L.drill = null; refresh(); };
let searchTimer;
$('#search').oninput = e => { clearTimeout(searchTimer); searchTimer = setTimeout(() => { L.q = e.target.value.trim(); L.drill = null; refresh(); }, 140); };

/* ---------- Playback ---------- */
// The demo has no audio files, so a clock stands in for the <audio> element.
class FakeAudio extends EventTarget {
  constructor() {
    super(); this.paused = true; this.currentTime = 0; this.duration = 0;
    setInterval(() => {
      if (this.paused || !this.duration) return;
      this.currentTime += .1; this.dispatchEvent(new Event('timeupdate'));
      if (this.currentTime >= this.duration) { this.paused = true; this.dispatchEvent(new Event('ended')); }
    }, 100);
  }
  play() { this.paused = false; this.dispatchEvent(new Event('play')); return Promise.resolve(); }
  pause() { this.paused = true; this.dispatchEvent(new Event('pause')); }
}
const au = DEMO ? new FakeAudio() : new Audio();
// an: null = still measuring, false = this format can't be decoded for analysis, object = measurements
const P = { t: null, q: null, listened: 0, started: 0, last: 0, open: false, an: null, info: null, raf: 0 };
const duration = () => (au.duration && isFinite(au.duration) ? au.duration : P.t ? P.t.ms / 1000 : 0);

function logPlay(completed) {
  if (P.t && P.listened > 1000) {
    call('log_play', { track_id: P.t.id, started: P.started, offset_min: -new Date().getTimezoneOffset(), ms_played: Math.round(P.listened), completed }).catch(() => {});
  }
  P.listened = 0;
}
async function play(t, q) {
  logPlay(false);
  P.t = t; if (q) P.q = q;
  P.started = Math.floor(Date.now() / 1000); P.last = 0;
  if (t.codec === 'DST') toast('This file is DST-compressed DSD, which Sleeve can\u2019t decode yet.', true);
  if (DEMO) { au.duration = t.ms / 1000; au.currentTime = 0; }
  else {
    // DSD can't be opened as a plain file by the webview: the core streams it as PCM instead.
    const src = MODE !== 'tauri' ? '/stream/' + t.id : isDsd(t) ? (await call('stream_url', { track_id: t.id })).url : T.core.convertFileSrc(t.path);
    if (P.t !== t) return;
    au.src = src;
  }
  au.play().catch(() => toast(`Can't play ${t.title}. The file may have moved; scan the folder again.`));
  $('#mini').hidden = false;
  $('#mini-title').textContent = t.title;
  $('#mini-sub').textContent = t.artist;
  $('#se-track').textContent = `${t.title} — ${t.artist}`;
  paintNow(t); list.redraw(); shareSync();
  if ('mediaSession' in navigator) navigator.mediaSession.metadata = new MediaMetadata({ title: t.title, artist: t.artist, album: t.album });
  call('similar', { track_id: t.id }).then(rows => {
    if (P.t !== t) return;
    P.sim = rows;
    $('#pl-similar').innerHTML = rows.length ? rows.map(trackRow).join('') : '<div class="empty">Nothing similar yet. Similar tracks come from shared genre tags.</div>';
  }).catch(() => {});
}
async function step(d) {
  const q = P.q;
  if (!q) return;
  if (d < 0 && au.currentTime > 3) return void (au.currentTime = P.last = 0);
  const i = q.i + d;
  if (i < 0 || i >= q.total) return;
  const off = Math.floor(i / 100) * 100;
  if (q.off !== off) { q.rows = (await q.fetch(off, 100)).rows; q.off = off; }
  q.i = i;
  if (q.rows[i - off]) play(q.rows[i - off]);
}
const toggle = () => { if (P.t) au.paused ? au.play().catch(fail) : au.pause(); };

/* Now playing: cover, spec plate, waveform, measurements */
const css = getComputedStyle(document.documentElement), FG = css.getPropertyValue('--fg').trim(), BG = css.getPropertyValue('--bg').trim(),
  FAINT = css.getPropertyValue('--faint').trim(), DIM = css.getPropertyValue('--dim').trim(), SANS = css.getPropertyValue('--sans');
const bytes = n => n >= 1e9 ? (n / 1e9).toFixed(2) + ' GB' : (n / 1e6).toFixed(1) + ' MB';
const dB = (v, unit = 'dBFS') => (v <= -119 ? '−∞' : (v < -.049 ? '−' : v > .049 ? '+' : '') + Math.abs(v).toFixed(1)) + ' ' + unit;
const date = s => new Date(s * 1000).toLocaleDateString('en-GB', { day: '2-digit', month: 'short', year: 'numeric' });
const cell = (k, v, cls = '') => `<div class="cell ${cls}"><span class="lab">${k}</span><b>${v}</b></div>`;

// Placeholder sleeve when a file has no artwork: one of three plain layouts, fixed per album.
function art(cv, t) {
  const c = cv.getContext('2d'), S = 600;
  let s = (t.album_id + 1) * 2654435761 >>> 0;
  const r = () => (s = Math.imul(s ^ s >>> 15, 2246822507) + 0x9E3779B9 >>> 0) / 4294967296;
  c.globalCompositeOperation = 'source-over';
  c.fillStyle = FG; c.fillRect(0, 0, S, S); c.fillStyle = c.strokeStyle = BG;
  const kind = Math.floor(r() * 3);
  if (kind === 0) {
    c.beginPath(); c.arc(S * (.5 + r() * .2), S * (.52 + r() * .12), 150 + r() * 80, 0, 7); c.fill();
    c.beginPath(); c.arc(70 + r() * 80, 150 + r() * 60, 26 + r() * 30, 0, 7); c.fill();
  } else if (kind === 1) {
    c.lineWidth = 6 + r() * 10;
    for (let rad = 250; rad > 30; rad -= 30 + r() * 16) { c.beginPath(); c.arc(S / 2, S / 2 + 20, rad, 0, 7); c.stroke(); }
    c.beginPath(); c.arc(S / 2, S / 2 + 20, 14, 0, 7); c.fill();
  } else {
    for (let y = 0; y < 5; y++) for (let x = 0; x < 5; x++) { c.beginPath(); c.arc(100 + x * 100, 150 + y * 92, 6 + r() * 38, 0, 7); c.fill(); }
  }
  c.globalCompositeOperation = 'difference'; c.fillStyle = '#fff';
  c.font = '700 26px ' + SANS; c.textBaseline = 'top';
  c.fillText(t.artist.toUpperCase().slice(0, 30), 28, 28);
  c.font = '500 20px ' + SANS; c.fillText(t.album.toUpperCase().slice(0, 38), 28, 62);
  c.globalCompositeOperation = 'source-over';
}
function specs() {
  const t = P.t, i = P.info, a = P.an, ch = t.ch || 2, pcm = t.bits ? t.bits * t.rate * ch / 1000 : 0, wait = '···';
  const m = f => a ? f(a) : a === false ? 'n/a' : wait, h = f => i ? f(i) : wait;
  const group = (name, cells) => `<h3>(${name})</h3><div class="cells">${cells.join('')}</div>`;
  $('#np-specs').innerHTML =
    group('Signal', [cell('Codec', esc(t.codec)), cell('Class', isDsd(t) ? spec(t) + ' \u00b7 1 bit' : TIER[t.tier]), cell('Channels', ch + (ch === 1 ? ' \u00b7 mono' : ch === 2 ? ' \u00b7 stereo' : '')),
      ...(isDsd(t) ? [cell('Sample rate', (t.rate / 1e6).toFixed(4) + ' MHz'), cell('Plays as', `PCM 24/${+(t.rate / (Math.max(8, Math.floor(t.rate / 88200 / 8) * 8)) / 1000).toFixed(1)}`), cell('Stream', 'raw 1-bit' + (t.codec === 'DST' ? ' \u00b7 DST' : ''))]
        : [cell('Bandwidth', '0\u2013' + +(t.rate / 2000).toFixed(2) + ' kHz'), cell('Theoretical SNR', t.bits ? (6.02 * t.bits + 1.76).toFixed(1) + ' dB' : 'n/a'),
          cell('Size vs raw PCM', pcm ? (t.kbps / pcm * 100).toFixed(1) + '%' : 'n/a')])]) +
    group('Measured from the audio', [cell('Sample peak', m(a => dB(a.peak_db))), cell('RMS level', m(a => dB(a.rms_db))), cell('Crest factor', m(a => (a.peak_db - a.rms_db).toFixed(1) + ' dB')),
      cell('Dynamic range', m(a => a.dr == null ? 'n/a' : 'DR' + a.dr)), cell('Headroom', m(a => (-a.peak_db).toFixed(1) + ' dB')), cell('Clipped samples', m(a => num(a.clipped)), a && a.clipped ? 'bad' : '')]) +
    group('File', [cell('Size', bytes(t.size || 0)), cell('Length', mmss(t.ms) + '.' + String(t.ms % 1000).padStart(3, '0')), cell('Data rate', (t.size / 1e6 / (t.ms / 60000 || 1)).toFixed(1) + ' MB/min'),
      cell('Track', h(i => i.track_no ? `${two(i.track_no)} \u00b7 disc ${i.disc_no}` : 'untagged')), cell('Year', t.year || 'n/a'), cell('Genre', h(i => esc(i.genres || 'untagged'))),
      cell('Path', esc(t.path), 'wide')]);
}
function loadCover(t) {
  const img = $('#np-img');
  img.hidden = true;
  call('cover', { album_id: t.album_id }).then(r => {
    const u = coverUrl(r, t.album_id);
    if (P.t !== t || !u) return;
    img.onload = () => { if (P.t === t) img.hidden = false; };
    img.src = u;
  }).catch(() => {});
}
function paintNow(t) {
  P.an = null; P.info = null;
  $('#pl-title').textContent = t.title;
  $('#pl-sub').textContent = `${t.artist} — ${t.album}${t.year ? ' · ' + t.year : ''}`;
  $('#np-queue').textContent = P.q ? `(Now playing · ${num(P.q.i + 1)} / ${num(P.q.total)})` : '(Now playing)';
  $('#np-khz').textContent = isDsd(t) ? (t.rate / 1e6).toFixed(1) : +(t.rate / 1000).toFixed(1);
  $('#np-khz-u').textContent = isDsd(t) ? 'MHz' : 'kHz';
  $('#np-bit').textContent = t.bits || '—';
  $('#np-kbps').textContent = num(t.kbps);
  $('#np-tier').innerHTML = `${dots(t.tier)}${TIER[t.tier]} \u00b7 ${esc(isDsd(t) ? spec(t) : t.codec)}`;
  $('#np-level').textContent = 'Measuring…';
  art($('#np-art'), t); pop($('#np-cover')); loadCover(t);
  call('track_info', { track_id: t.id }).then(i => { if (P.t === t) { P.info = i; specs(); } }).catch(() => {});
  call('analyze', { track_id: t.id }).then(a => { if (P.t === t) { P.an = a; specs(); live(); } }, () => { if (P.t === t) { P.an = false; specs(); live(); } });
  specs(); live();
}
const wave = $('#np-wave'), wctx = wave.getContext('2d');
function live() {
  const t = P.t;
  if (!t || !P.open) return;
  const w = wave.clientWidth, h = wave.clientHeight, dpr = Math.min(2, devicePixelRatio || 1);
  if (wave.width !== Math.round(w * dpr)) { wave.width = Math.round(w * dpr); wave.height = Math.round(h * dpr); }
  wctx.setTransform(dpr, 0, 0, dpr, 0, 0); wctx.clearRect(0, 0, w, h);
  const dur = duration(), f = dur ? Math.min(1, au.currentTime / dur) : 0, n = Math.floor(w / 3), pk = P.an && P.an.peaks, mid = (h - 12) / 2, px = f * w;
  for (let i = 0; i < n; i++) {
    let v = .012;
    if (pk) { v = 0; for (let k = Math.floor(i * pk.length / n), z = Math.max(k + 1, Math.floor((i + 1) * pk.length / n)); k < z; k++) v = Math.max(v, pk[k]); v /= 255; }
    const bh = Math.max(1, v * (h - 18));
    wctx.fillStyle = i * 3 < px ? FG : FAINT; wctx.fillRect(i * 3, mid - bh / 2, 2, bh);
  }
  wctx.fillStyle = DIM; // ruler: a tick every 10 s, a taller one each minute
  for (let s = 0, st = dur > 900 ? 60 : 10; dur && s <= dur; s += st) wctx.fillRect(Math.round(s / dur * (w - 1)), h - (s % 60 ? 4 : 9), 1, s % 60 ? 4 : 9);
  wctx.fillStyle = FG; wctx.fillRect(Math.round(px), 0, 1, h - 10);
  wctx.beginPath(); wctx.arc(Math.round(px) + .5, 3.5, 3.5, 0, 7); wctx.fill();
  $('#pl-cur').textContent = mmss(au.currentTime * 1000);
  $('#pl-rem').textContent = '−' + mmss((dur - au.currentTime) * 1000);
  $('#np-sample').textContent = `Sample ${num(Math.floor(au.currentTime * t.rate))}`;
  if (pk) { const v = pk[Math.min(pk.length - 1, Math.floor(f * pk.length))] / 255; $('#np-level').textContent = 'Level ' + dB(v ? 20 * Math.log10(v) : -120); }
  else if (P.an === false) $('#np-level').textContent = 'No decoder for this format';
}
function loop() { P.raf = 0; live(); if (P.open && !au.paused) P.raf = requestAnimationFrame(loop); }
const kick = () => { if (!P.raf) P.raf = requestAnimationFrame(loop); };
let notch = null;
function scrub(e) {
  const b = wave.getBoundingClientRect(), f = Math.max(0, Math.min(1, (e.clientX - b.left) / b.width)), dur = duration();
  if (!dur) return;
  au.currentTime = P.last = f * dur;
  if (Math.floor(f * 50) !== notch) { notch = Math.floor(f * 50); haptic('light'); } // a tick every 2% of the track
  live();
}
wave.onpointerdown = e => { wave.setPointerCapture(e.pointerId); notch = -1; scrub(e); };
wave.onpointermove = e => { if (notch !== null) scrub(e); };
wave.onpointerup = wave.onpointercancel = () => { notch = null; shareSync(); };
wave.onkeydown = e => { const d = { ArrowLeft: -5, ArrowRight: 5 }[e.key]; if (d && duration()) { au.currentTime = P.last = Math.max(0, Math.min(duration() - .1, au.currentTime + d)); live(); } };

au.addEventListener('timeupdate', () => {
  const d = au.currentTime - P.last;
  if (d > 0 && d < 1.5) P.listened += d * 1000;
  P.last = au.currentTime;
  const dur = duration();
  $('#mini-bar').style.width = (dur ? au.currentTime / dur * 100 : 0) + '%';
  $('#mini-time').textContent = mmss(au.currentTime * 1000);
});
au.addEventListener('ended', () => { logPlay(true); step(1); });
for (const ev of ['play', 'pause']) au.addEventListener(ev, () => {
  $('#mini-toggle').classList.toggle('playing', !au.paused);
  $('#pl-toggle').textContent = au.paused ? 'Play' : 'Pause';
  pop($('#pl-toggle')); pop($('#mini-toggle'));
  kick(); shareSync();
});
addEventListener('pagehide', () => logPlay(false));
$('#pl-toggle').onclick = toggle; $('#pl-prev').onclick = () => step(-1); $('#pl-next').onclick = () => step(1);
if ('mediaSession' in navigator) {
  const ms = navigator.mediaSession;
  ms.setActionHandler('play', toggle); ms.setActionHandler('pause', toggle);
  ms.setActionHandler('previoustrack', () => step(-1)); ms.setActionHandler('nexttrack', () => step(1));
}
const sheet = (id, open) => {
  const el = $(id);
  el.classList.toggle('open', open); el.inert = !open;
  if (id === '#player') { P.open = open; if (open) kick(); }
};
$('#mini').onclick = e => e.target.closest('#mini-toggle') ? toggle() : sheet('#player', true);
$('#pl-close').onclick = () => sheet('#player', false);
$('#pl-similar').onclick = e => {
  const el = e.target.closest('[data-i]');
  if (el) { const rows = P.sim; play(rows[+el.dataset.i], { fetch: async () => ({ rows, total: rows.length }), total: rows.length, i: +el.dataset.i }); }
};
new ResizeObserver(live).observe(wave);

/* ---------- Pop-up panel: menus and small forms ---------- */
let popActs = [];
function openPop(title, html) { $('#pop-title').textContent = `(${title})`; $('#pop-body').innerHTML = html; $('#pop').hidden = false; }
const closePop = () => { $('#pop').hidden = true; };
$('#pop-close').onclick = $('.pop-bg').onclick = closePop;
const actsHtml = items => items.map((it, i) => `<button class="act${it.danger ? ' danger' : ''}" data-a="${i}"><b>${esc(it.label)}</b>${it.sub ? `<span>${esc(it.sub)}</span>` : ''}</button>`).join('');
function menu(title, note, items) { popActs = items; openPop(title, (note ? `<p class="note">${esc(note)}</p>` : '') + actsHtml(items)); }
$('#pop-body').addEventListener('click', e => {
  const b = e.target.closest('[data-a]');
  if (b) { closePop(); Promise.resolve(popActs[+b.dataset.a].run()).catch(fail); }
});
function form(title, fields, save, label = 'Save') {
  openPop(title, `<form id="pop-form">${fields.map(f => `<label class="lab" for="f-${f.key}">${f.label}</label><input id="f-${f.key}" name="${f.key}" type="text"${f.num ? ' inputmode="numeric"' : ''} value="${esc(f.value ?? '')}" autocomplete="off" spellcheck="false">`).join('')}<button class="btn" type="submit">${label}</button></form>`);
  $('#pop-form').onsubmit = async e => {
    e.preventDefault();
    try { await save(Object.fromEntries(new FormData(e.target))); closePop(); haptic('medium'); } catch (err) { fail(err); }
  };
}

/* ---------- Managing tracks, albums and playlists ---------- */
function goto(drill) { sheet('#player', false); L.drill = drill; show('library'); refresh(); }
function applyEdit(res) {
  toast(DEMO ? 'Saved. This is demo data, so no file was changed.' : res.written ? 'Saved to the file’s tags' : 'Saved in Sleeve only. This file can’t be written to.');
  const t = res.track;
  if (P.t && P.t.id === t.id) {
    P.t = t;
    $('#mini-title').textContent = t.title; $('#mini-sub').textContent = t.artist; $('#se-track').textContent = `${t.title} — ${t.artist}`;
    paintNow(t);
  }
  loadSummary(); refresh(); statsReady = false;
}
async function trackMenu(t, now) {
  let i = null;
  try { i = await call('track_info', { track_id: t.id }); } catch {}
  const items = [
    { label: 'Add to playlist', run: () => pickPlaylist(t) },
    { label: 'Edit details', sub: 'Title, artist, album, year, track number, genre', run: () => editTrack(t, i) },
    { label: 'Move to another album', sub: `Now on ${t.album}`, run: () => pickAlbum(t) },
    { label: 'Go to album', sub: t.album, run: () => goto({ kind: 'album', id: t.album_id, name: t.album }) },
    { label: 'Go to artist', sub: t.artist, run: () => goto({ kind: 'artist', id: t.artist_id, name: t.artist }) },
  ];
  if (now) items.unshift({ label: 'Listen together', sub: 'Open a timed link to what you are playing', run: () => { sheet('#player', false); show('session'); } });
  if (L.drill?.kind === 'playlist' && t.item) items.push({ label: 'Remove from this playlist', danger: true, run: async () => { await call('playlist_remove', { id: L.drill.id, item: t.item }); refresh(); } });
  menu(t.title, i ? `Played ${num(i.plays)} times${i.last_played ? ' · last ' + date(i.last_played) : ''} · added ${date(i.added)}` : '', items);
}
function editTrack(t, i) {
  form('Edit details', [{ key: 'title', label: 'Title', value: t.title }, { key: 'artist', label: 'Artist', value: t.artist }, { key: 'album', label: 'Album', value: t.album },
    { key: 'year', label: 'Year', value: t.year || '', num: 1 }, { key: 'track_no', label: 'Track number', value: i?.track_no || '', num: 1 }, { key: 'genres', label: 'Genre (comma separated)', value: i?.genres || '' }],
  async v => applyEdit(await call('edit_track', { track_id: t.id, title: v.title, artist: v.artist, album: v.album, year: +v.year || null, track_no: +v.track_no || null, genres: v.genres })));
}
function pickAlbum(t) {
  openPop('Move to album', `<p class="note">${esc(t.title)}</p><input id="alb-q" type="search" placeholder="Search albums" autocomplete="off"><div id="alb-res"></div>`);
  let timer;
  const find = async () => {
    const r = await call('albums', { q: $('#alb-q')?.value.trim() || '', limit: 30 });
    if (!$('#alb-res')) return;
    popActs = r.rows.map(al => ({ label: al.title, sub: `${al.artist}${al.year ? ' · ' + al.year : ''} · ${al.n} tracks`,
      run: async () => applyEdit(await call('edit_track', { track_id: t.id, album: al.title, album_artist: al.artist })) }));
    $('#alb-res').innerHTML = actsHtml(popActs) || '<div class="empty">No album matches.</div>';
  };
  $('#alb-q').oninput = () => { clearTimeout(timer); timer = setTimeout(() => find().catch(fail), 140); };
  find().catch(fail);
}
async function pickPlaylist(t) {
  const add = async p => { const r = await call('playlist_add', { id: p.id, track_id: t.id }); toast(r.added ? `Added to ${p.name}` : `Already in ${p.name}`); if (L.tab === 'playlists') refresh(); };
  const r = await call('playlists', { limit: 200 });
  menu('Add to playlist', t.title, [
    { label: '+ New playlist', run: () => form('New playlist', [{ key: 'name', label: 'Name' }], async v => add(await call('playlist_create', { name: v.name })), 'Create and add') },
    ...r.rows.map(p => ({ label: p.name, sub: `${num(p.n)} tracks`, run: () => add(p) })),
  ]);
}
$('#pl-more').onclick = () => P.t && trackMenu(P.t, true);
$('#pl-new').onclick = () => form('New playlist', [{ key: 'name', label: 'Name' }], async v => { await call('playlist_create', { name: v.name }); refresh(); }, 'Create');
function editAlbum(d) {
  const al = d.al;
  form('Edit album', [{ key: 'title', label: 'Album title', value: al.title }, { key: 'artist', label: 'Album artist', value: al.artist },
    { key: 'year', label: 'Year', value: al.year || '', num: 1 }, { key: 'genres', label: 'Genre for every track (comma separated)', value: al.genres || '' }], async v => {
    // Only touch genre when it was changed, so per-track genres aren't flattened by accident.
    const r = await call('edit_album', { album_id: d.id, title: v.title, artist: v.artist, year: +v.year || null, genres: v.genres !== (al.genres || '') ? v.genres : null });
    toast(DEMO ? 'Album saved. Demo data, so no file was changed.' : `Album saved. ${r.written} of ${r.tracks} files updated.`);
    if (P.t && P.t.album_id === d.id) { P.t.album = v.title; P.t.album_id = r.album_id; paintNow(P.t); }
    thumbs.delete(d.id); thumbs.delete(r.album_id);
    L.drill = { kind: 'album', id: r.album_id, name: v.title };
    loadSummary(); refresh(); statsReady = false;
  });
}
// Any image the browser can open is accepted; it is scaled to at most 1400 px and stored as JPEG.
$('#cover-file').onchange = async e => {
  const file = e.target.files[0], d = L.drill;
  e.target.value = '';
  if (!file || d?.kind !== 'album') return;
  try {
    const bmp = await createImageBitmap(file), k = Math.min(1, 1400 / Math.max(bmp.width, bmp.height));
    const cv = Object.assign(document.createElement('canvas'), { width: Math.round(bmp.width * k), height: Math.round(bmp.height * k) });
    cv.getContext('2d').drawImage(bmp, 0, 0, cv.width, cv.height);
    const r = await call('set_cover', { album_id: d.id, data: cv.toDataURL('image/jpeg', .9).split(',')[1] });
    toast(DEMO ? 'Cover changed. Demo data, so no file was changed.' : `Cover changed. Embedded in ${r.written} of ${r.tracks} files.`);
    thumbs.delete(d.id); haptic('medium'); paintDrill();
    if (P.t && P.t.album_id === d.id) loadCover(P.t);
  } catch (err) { toast(String(err?.message || err).startsWith('Use') ? err.message : 'That image could not be opened. Try a JPEG or PNG.'); }
};
$('#dh-cover').onclick = () => $('#cover-file').click();
$('#crumb').onclick = () => { L.drill = null; refresh(); };
$('#drill-edit').onclick = () => {
  const d = L.drill;
  if (d.kind === 'album') {
    if (d.al) menu(d.al.title, 'Album', [
      { label: 'Edit name and details', sub: 'Title, album artist, year, genre', run: () => editAlbum(d) },
      { label: 'Change cover', sub: 'Pick an image from this device', run: () => $('#cover-file').click() }]);
  } else menu(d.name, 'Playlist', [
    { label: 'Rename', run: () => form('Rename playlist', [{ key: 'name', label: 'Name', value: d.name }], async v => { const p = await call('playlist_rename', { id: d.id, name: v.name }); L.drill = { ...d, name: p.name }; refresh(); }) },
    { label: 'Delete playlist', sub: 'The tracks stay in your library', danger: true, run: () => menu(`Delete ${d.name}?`, 'The tracks stay in your library.', [
      { label: 'Delete playlist', danger: true, run: async () => { await call('playlist_delete', { id: d.id }); L.drill = null; refresh(); } }, { label: 'Keep it', run: () => {} }]) },
  ]);
};

/* ---------- Stats ---------- */
const S = { days: 365, f: null, season: 'month', d: null };
const DOW = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'], DAY = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'];
const MON = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
const MONTH = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];
// Wet/dry follows Indonesia's monsoon; four seasons are northern-hemisphere meteorological.
const SEASONS = {
  wetdry: [['Wet · Nov–Mar', [10, 11, 0, 1, 2], 'the wet season'], ['Dry · Apr–Oct', [3, 4, 5, 6, 7, 8, 9], 'the dry season']],
  four: [['Dec–Feb', [11, 0, 1], 'Dec–Feb'], ['Mar–May', [2, 3, 4], 'Mar–May'], ['Jun–Aug', [5, 6, 7], 'Jun–Aug'], ['Sep–Nov', [8, 9, 10], 'Sep–Nov']],
};
function bars(el, vals, label) {
  const max = Math.max(...vals), pk = max > 0 ? vals.indexOf(max) : -1;
  el.style.setProperty('--n', vals.length);
  el.innerHTML = vals.map((v, i) => `<div class="bar${i === pk ? ' pk' : ''}" style="--i:${i}">${i === pk ? `<b style="bottom:calc(${v / max * 100}% + 4px)">${num(Math.round(v))}</b>` : ''}<i style="height:${max ? (v / max * 100).toFixed(1) : 0}%"></i><span>${label(i)}</span></div>`).join('');
  return pk;
}
function tops(el, rows, kind) {
  const max = rows[0]?.plays || 1;
  el.innerHTML = rows.length ? rows.map((r, i) => `<button class="top" data-kind="${kind}" data-id="${r.id}" data-name="${esc(r.name)}" aria-pressed="${!!S.f && S.f.kind === kind && S.f.id === r.id}" style="--w:${(r.plays / max * 100).toFixed(1)}%"><em>${two(i + 1)}</em><span class="rt"><b>${esc(r.name)}</b>${r.sub ? `<span>${esc(r.sub)}</span>` : ''}</span><strong>${num(r.plays)}</strong></button>`).join('')
    : '<div class="empty">Nothing played in this range.</div>';
}
function paintStats() {
  const d = S.d, none = !d.plays, hours = d.ms / 3.6e6;
  countTo($('#st-plays'), d.plays, v => num(Math.round(v)));
  countTo($('#st-hours'), hours, v => hours < 100 ? v.toFixed(1) : num(Math.round(v)));
  $('#st-filter').hidden = !S.f;
  if (S.f) $('#st-filter').textContent = `(Only ${S.f.name} · clear)`;
  const ph = bars($('#c-hour'), d.by_hour, i => i % 6 ? '' : two(i));
  $('#pk-hour').textContent = none ? 'No plays yet' : `Most at ${two(ph)}:00`;
  const pd = bars($('#c-dow'), d.by_dow, i => DOW[i]);
  $('#pk-dow').textContent = none ? '' : `Most on ${DAY[pd]}s`;
  if (S.season === 'month') {
    const pm = bars($('#c-month'), d.by_month, i => MON[i][0]);
    $('#pk-month').textContent = none ? '' : `Most in ${MONTH[pm]}`;
  } else {
    const g = SEASONS[S.season], pm = bars($('#c-month'), g.map(s => s[1].reduce((a, m) => a + d.by_month[m], 0) / s[1].length), i => g[i][0]);
    $('#pk-month').textContent = none ? '' : `Most in ${g[pm][2]} · plays per month`;
  }
  const max = Math.max(1, ...d.punch), pp = d.punch.indexOf(max);
  $('#pk-punch').textContent = none ? '' : `Busiest ${DOW[Math.floor(pp / 24)]} ${two(pp % 24)}:00`;
  let h = '';
  for (let day = 0; day < 7; day++) {
    h += `<span>${DOW[day]}</span>`;
    for (let hr = 0; hr < 24; hr++) { const c = d.punch[day * 24 + hr]; h += c ? `<u><i style="--s:${(.2 + .8 * Math.sqrt(c / max)).toFixed(2)};--i:${day * 4 + hr * 3}"></i></u>` : '<u class="z"><i></i></u>'; }
  }
  $('#c-punch').innerHTML = h + '<span></span>' + [0, 6, 12, 18].map(x => `<span style="grid-column:span 6">${two(x)}</span>`).join('');
  tops($('#top-tracks'), d.top_tracks, 'track'); tops($('#top-artists'), d.top_artists, 'artist'); tops($('#top-genres'), d.top_genres, 'genre');
}
async function loadStats() {
  S.d = await call('dash', { since: S.days ? Math.floor(Date.now() / 1000) - S.days * 86400 : 0, kind: S.f?.kind, id: S.f?.id });
  paintStats();
}
$('#st-range').onclick = e => { const b = e.target.closest('button'); if (b) { press($('#st-range'), b); S.days = +b.dataset.d; loadStats().catch(fail); } };
$('#st-season').onclick = e => { const b = e.target.closest('button'); if (b) { press($('#st-season'), b); S.season = b.dataset.m; paintStats(); } };
$('#st-filter').onclick = () => { S.f = null; loadStats().catch(fail); };
document.querySelector('.tops').onclick = e => {
  const b = e.target.closest('.top');
  if (!b) return;
  const f = { kind: b.dataset.kind, id: +b.dataset.id, name: b.dataset.name };
  S.f = S.f && S.f.kind === f.kind && S.f.id === f.id ? null : f;
  loadStats().then(() => S.f && $('#v-stats').scrollTo({ top: 0, behavior: 'smooth' })).catch(fail);
};

/* Genre sphere: every genre is a dot on a globe; genres that share artists are pulled together. */
function Sphere(cv, info) {
  const ctx = cv.getContext('2d');
  const LINE = FAINT, hint = info.textContent;
  let pts = [], links = [], pr = [], ry = .5, rx = -.4, drag = null, run = false, sel = -1, W = 0, maxT = 1, maxP = 1;
  function set(data) {
    const g = data.genres.slice(0, 48), n = g.length, ix = new Map(g.map((x, i) => [x.id, i]));
    pts = g.map((x, i) => { const y = 1 - 2 * (i + .5) / n, r = Math.sqrt(1 - y * y), a = i * 2.399963; return { ...x, rank: i, x: Math.cos(a) * r, y, z: Math.sin(a) * r }; });
    links = data.links.filter(l => ix.has(l[0]) && ix.has(l[1])).map(l => [ix.get(l[0]), ix.get(l[1]), l[2]]);
    const mw = Math.max(1, ...links.map(l => l[2]));
    for (let it = 0; it < 140; it++) {
      const F = pts.map(() => [0, 0, 0]);
      for (let i = 0; i < n; i++) for (let j = i + 1; j < n; j++) {
        const a = pts[i], b = pts[j], dx = a.x - b.x, dy = a.y - b.y, dz = a.z - b.z, k = .012 / (dx * dx + dy * dy + dz * dz + .01);
        F[i][0] += dx * k; F[i][1] += dy * k; F[i][2] += dz * k; F[j][0] -= dx * k; F[j][1] -= dy * k; F[j][2] -= dz * k;
      }
      for (const [i, j, w] of links) {
        const a = pts[i], b = pts[j], k = .06 * w / mw, dx = (b.x - a.x) * k, dy = (b.y - a.y) * k, dz = (b.z - a.z) * k;
        F[i][0] += dx; F[i][1] += dy; F[i][2] += dz; F[j][0] -= dx; F[j][1] -= dy; F[j][2] -= dz;
      }
      pts.forEach((p, i) => { p.x += F[i][0]; p.y += F[i][1]; p.z += F[i][2]; const m = Math.hypot(p.x, p.y, p.z) || 1; p.x /= m; p.y /= m; p.z /= m; });
    }
    maxT = Math.max(1, ...pts.map(p => p.tracks)); maxP = Math.max(1, ...pts.map(p => p.plays));
    draw();
  }
  function draw() {
    const w = cv.clientWidth;
    if (!w) return;
    const dpr = Math.min(2, devicePixelRatio || 1);
    if (w !== W) { W = w; cv.width = cv.height = Math.round(w * dpr); }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0); ctx.clearRect(0, 0, W, W);
    const c = W / 2, R = W * .34, cy = Math.cos(ry), sy = Math.sin(ry), cx = Math.cos(rx), sx = Math.sin(rx);
    ctx.lineWidth = 1; ctx.strokeStyle = LINE; ctx.globalAlpha = 1;
    ctx.beginPath(); ctx.arc(c, c, R, 0, 7); ctx.stroke();
    ctx.beginPath(); ctx.ellipse(c, c, R, R * Math.abs(sx), 0, 0, 7); ctx.stroke();
    pr = pts.map((p, i) => {
      const x = p.x * cy + p.z * sy, z0 = -p.x * sy + p.z * cy, y = p.y * cx - z0 * sx, z = p.y * sx + z0 * cx;
      return { i, p, x: c + x * R, y: c - y * R, z, r: (3 + 13 * Math.sqrt(p.tracks / maxT)) * (.62 + .19 * (z + 1)) };
    });
    ctx.strokeStyle = FG; ctx.globalAlpha = .18;
    for (const [a, b] of links) if (pr[a].z > -.1 && pr[b].z > -.1) { ctx.beginPath(); ctx.moveTo(pr[a].x, pr[a].y); ctx.lineTo(pr[b].x, pr[b].y); ctx.stroke(); }
    ctx.font = '500 10px ' + SANS;
    for (const d of pr.slice().sort((a, b) => a.z - b.z)) {
      const far = d.z < 0 ? .3 : 1;
      ctx.beginPath(); ctx.arc(d.x, d.y, d.r, 0, 7);
      ctx.globalAlpha = 1; ctx.fillStyle = BG; ctx.fill();
      ctx.globalAlpha = far * d.p.plays / maxP; ctx.fillStyle = FG; ctx.fill();
      ctx.globalAlpha = far; ctx.lineWidth = d.i === sel ? 2.5 : 1; ctx.stroke();
      if (d.z > 0 && (d.p.rank < 14 || d.i === sel)) { ctx.globalAlpha = .55 + .45 * d.z; ctx.fillText(d.p.name.toUpperCase(), d.x + d.r + 5, d.y + 3.5); }
    }
  }
  function tick() { if (!run) return; if (!drag && !still) ry += .003; draw(); requestAnimationFrame(tick); }
  cv.onpointerdown = e => { drag = { x: e.clientX, y: e.clientY, moved: 0 }; cv.setPointerCapture(e.pointerId); };
  cv.onpointermove = e => {
    if (!drag) return;
    const dx = e.clientX - drag.x, dy = e.clientY - drag.y;
    drag.moved += Math.abs(dx) + Math.abs(dy); drag.x = e.clientX; drag.y = e.clientY;
    ry += dx * .01; rx = Math.max(-1.3, Math.min(1.3, rx + dy * .01));
    if (!run || still) draw();
  };
  cv.onpointerup = cv.onpointercancel = e => {
    if (drag && drag.moved < 6) {
      const b = cv.getBoundingClientRect(), x = e.clientX - b.left, y = e.clientY - b.top;
      const hit = pr.filter(d => d.z > 0 && Math.hypot(d.x - x, d.y - y) < d.r + 12).sort((a, b) => Math.hypot(a.x - x, a.y - y) - Math.hypot(b.x - x, b.y - y))[0];
      sel = hit ? hit.i : -1;
      if (hit) haptic('medium');
      info.textContent = hit ? `${hit.p.name} · ${num(hit.p.tracks)} tracks · ${num(hit.p.plays)} plays` : hint;
      info.classList.toggle('hi', !!hit);
      draw();
    }
    drag = null;
  };
  // Only animate while the sphere is actually on screen.
  new IntersectionObserver(([en]) => { const on = en.isIntersecting; if (on && !run) { run = true; still ? draw() : tick(); } else if (!on) run = false; }).observe(cv);
  return { set };
}
const sphere = Sphere($('#sphere'), $('#sphere-info'));

/* ---------- Listen-together session ---------- */
let SH = { active: false }, mins = 30;
function paintSession() {
  $('#se-idle').hidden = SH.active; $('#se-live').hidden = !SH.active;
  $('#se-start').disabled = !P.t;
  $('#se-start').textContent = P.t ? 'Open session' : 'Play a track first';
  if (!SH.active) return;
  $('#se-url').textContent = SH.url; $('#se-guests').textContent = SH.guests;
  $('#se-left').textContent = mmss(Math.max(0, SH.expires * 1000 - Date.now()));
}
async function shareSync() {
  if (!SH.active) return paintSession();
  try { SH = P.t ? await call('share_sync', { track_id: P.t.id, pos_ms: Math.round(au.currentTime * 1000), playing: !au.paused }) : await call('share_status'); } catch {}
  paintSession();
}
setInterval(() => { if (SH.active) { paintSession(); if (Date.now() % 3000 < 1000) shareSync(); } }, 1000);
$('#se-mins').onclick = e => { const b = e.target.closest('button'); if (b) { press($('#se-mins'), b); mins = +b.dataset.m; } };
$('#se-start').onclick = async () => {
  try { SH = await call('share_start', { track_id: P.t.id, minutes: mins, pos_ms: Math.round(au.currentTime * 1000), playing: !au.paused }); paintSession(); haptic('heavy'); pop($('#se-left')); } catch (e) { fail(e); }
};
$('#se-stop').onclick = async () => { try { SH = await call('share_stop'); } catch (e) { fail(e); } paintSession(); };
$('#se-copy').onclick = () => navigator.clipboard.writeText(SH.url).then(() => toast('Link copied'), () => toast('Press and hold the link to copy it'));

/* ---------- Scan ---------- */
$('#scan-open').onclick = () => sheet('#scan', true);
$('#scan-close').onclick = () => sheet('#scan', false);
$('#scan-go').onclick = async () => {
  const path = $('#scan-path').value.trim();
  try { await call('scan_start', { path }); } catch (e) { return fail(e); }
  store.set('sleeve.path', path);
  $('#scan-go').disabled = true;
  const timer = setInterval(async () => {
    const s = await call('scan_status');
    $('#scan-count').textContent = s.running ? `${num(s.done)} / ${num(s.total)}` : `${num(s.added)} added`;
    $('#scan-msg').textContent = [s.message, s.removed ? `${num(s.removed)} removed` : '', s.errors ? `${num(s.errors)} unreadable` : ''].filter(Boolean).join(' · ');
    if (s.running) return;
    clearInterval(timer); $('#scan-go').disabled = false;
    loadSummary(); refresh(); statsReady = false;
  }, 400);
};

/* ---------- Tabs and start-up ---------- */
let statsReady = false;
function show(view) {
  for (const v of ['library', 'stats', 'session']) $('#v-' + v).hidden = v !== view;
  const el = $('#v-' + view);
  el.classList.remove('enter'); void el.offsetWidth; el.classList.add('enter');
  document.querySelectorAll('#tabs button').forEach(b => b.setAttribute('aria-pressed', b.dataset.view === view));
  if (view === 'stats') {
    loadStats().catch(fail);
    if (!statsReady) { statsReady = true; call('genre_map').then(sphere.set).catch(fail); }
  }
  if (view === 'session') shareSync();
  if (view === 'library') list.redraw();
}
$('#tabs').onclick = e => { const b = e.target.closest('button'); if (b) show(b.dataset.view); };

$('#demo').hidden = !DEMO;
$('#scan-path').value = store.get('sleeve.path') || '';
if (MODE === 'tauri' && !$('#scan-path').value) T.core.invoke('music_dir').then(p => ($('#scan-path').value = p)).catch(() => {});
loadSummary().catch(fail);
refresh();
const start = location.hash.slice(1);
if (start === 'stats' || start === 'session') show(start);
