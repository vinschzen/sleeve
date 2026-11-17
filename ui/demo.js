// Demo backend: an invented library so the UI can run in a plain browser with no Rust core.
// It answers the same commands as sleeve-core's `dispatch`. Nothing here ships data about real music.
window.Demo = (() => {
  let seed = 20261113;
  const rnd = () => { seed = seed + 0x6D2B79F5 | 0; let t = Math.imul(seed ^ seed >>> 15, 1 | seed); t = t + Math.imul(t ^ t >>> 7, 61 | t) ^ t; return ((t ^ t >>> 14) >>> 0) / 4294967296; };
  const pick = a => a[Math.floor(rnd() * a.length)];
  const A1 = 'Halde Korr Mara Tin Nine Vesper Ostro Luma Pale Brine Sable Quiet Iron Glass Velvet North Low Saint Ember Hollow Ardent Kite Opal Rust Silt Tall Wren Yarrow Zephyr Mono'.split(' ');
  const A2 = 'Harbor Lanterns Vey Atlas Circuit Orchard Signal Tides Assembly Engine Parade Static Garden Machine Choir Units Motel Archive Weather Bureau Index Relay Quartet Ensemble System'.split(' ');
  const W1 = 'Slow Blue Second Paper Night Soft Last Open Distant Small Bright Empty Warm Black Silver Early Long Lost New Still Wide Thin Deep Late Red Green Hidden Northern Southern Electric Quiet Burning Frozen Golden Plain Broken Inner Outer First Other Low High Near Far Minor Major Half Double'.split(' ');
  const W2 = 'Rooms Water Light Engines Summer Letters Fields Rivers Hours Maps Windows Signals Stations Coast Glass Houses Weather Tape Smoke Orbit Machines Mornings Wire Salt Harbors Circuits Gardens Mirrors Tunnels Islands Shadows Echoes Drift Static Motion Ritual Theory Season Distance Memory Pattern Current Shelter Horizon Margin Vapor'.split(' ');
  const GEN = ['Ambient', 'Drone', 'IDM', 'Techno', 'House', 'Disco', 'Funk', 'Soul', 'Hip-Hop', 'Trip-Hop', 'Dub', 'Jazz', 'Fusion', 'Bossa Nova', 'City Pop', 'Folk', 'Blues', 'Classical', 'Gamelan', 'Post-Rock', 'Shoegaze', 'Krautrock', 'Punk', 'Metal'];
  const SPEC = { 3: [['FLAC', 24, 96000], ['FLAC', 24, 192000], ['FLAC', 24, 48000], ['DSD', 1, 2822400], ['DSD', 1, 5644800]], 2: [['FLAC', 16, 44100], ['ALAC', 16, 44100]], 1: [['MP3', 0, 44100, 320]], 0: [['MP3', 0, 44100, 192], ['MP3', 0, 44100, 128]] };

  const artists = [], tracks = [], names = new Set();
  let albums = [];
  while (artists.length < 420) {
    const name = pick(A1) + ' ' + pick(A2);
    if (names.has(name)) continue;
    names.add(name);
    const g = Math.floor(rnd() * rnd() * GEN.length), gs = [g];
    if (rnd() < .6) gs.push((g + 1 + Math.floor(rnd() * 2)) % GEN.length);
    artists.push({ id: artists.length + 1, name, genres: gs, n: 0 });
  }
  for (const a of artists) {
    for (let k = 1 + Math.floor(rnd() * rnd() * 7); k > 0; k--) {
      const tier = [3, 3, 3, 2, 2, 2, 2, 2, 1, 1, 0][Math.floor(rnd() * 11)], s = pick(SPEC[tier]);
      const al = { id: albums.length + 1, title: rnd() < .3 ? pick(W2) : pick(W1) + ' ' + pick(W2), artist: a.name, year: 1968 + Math.floor(rnd() * 58), n: 6 + Math.floor(rnd() * 9), tier };
      albums.push(al);
      for (let n = 0; n < al.n; n++) {
        tracks.push({
          id: tracks.length + 1, title: rnd() < .25 ? pick(W2) : pick(W1) + ' ' + pick(W2), artist: a.name, album: al.title,
          ms: 120000 + Math.floor(rnd() * 330000), codec: s[0], bits: s[1], rate: s[2],
          kbps: s[3] || Math.round(s[1] * s[2] * 2 * (s[0] === 'DSD' ? 1 : .52 + rnd() * .14) / 1000), tier, path: `/Music/${a.name}/${al.title}/${String(n + 1).padStart(2, '0')}.${s[0] === 'MP3' ? 'mp3' : s[0] === 'ALAC' ? 'm4a' : s[0] === 'DSD' ? 'dsf' : 'flac'}`, year: al.year, artist_id: a.id, album_id: al.id, ch: 2, no: n + 1,
        });
        const t = tracks[tracks.length - 1];
        t.size = Math.round(t.ms / 8 * t.kbps);
        a.n++;
      }
    }
  }
  const cmp = (a, b) => a.title.localeCompare(b.title) || a.id - b.id;
  let sorted, albumSeq = albums.length;
  // Re-derive everything that depends on names after an edit.
  function reindex() {
    for (const x of albums) x.n = 0;
    for (const x of artists) x.n = 0;
    for (const t of tracks) { albums.find(x => x.id === t.album_id).n++; artists[t.artist_id - 1].n++; }
    albums = albums.filter(x => x.n).sort(cmp);
    sorted = { title: tracks.slice().sort(cmp), new: tracks.slice().reverse(), quality: tracks.slice().sort((a, b) => b.tier - a.tier || cmp(a, b)) };
  }
  reindex();
  const artistNamed = name => artists.find(x => x.name.toLowerCase() === name.toLowerCase()) || (artists.push({ id: artists.length + 1, name, genres: [], n: 0 }), artists[artists.length - 1]);
  function edit(t, a) {
    const v = k => (typeof a[k] === 'string' && a[k].trim()) || null, old = albums.find(x => x.id === t.album_id);
    if (v('title')) t.title = v('title');
    if (v('artist')) { const ar = artistNamed(v('artist')); if (ar.id !== t.artist_id) { if (!ar.genres.length) ar.genres = artists[t.artist_id - 1].genres.slice(); t.artist = ar.name; t.artist_id = ar.id; } }
    if (a.year) t.year = a.year;
    if (a.track_no) t.no = a.track_no;
    if (v('genres')) t.genres = v('genres');
    const title = v('album') || t.album, by = v('album_artist') || old.artist;
    let al = albums.find(x => x.title.toLowerCase() === title.toLowerCase() && x.artist.toLowerCase() === by.toLowerCase());
    if (!al) albums.push(al = { id: ++albumSeq, title, artist: by, year: t.year, n: 0, tier: t.tier });
    t.album = al.title; t.album_id = al.id;
    return t;
  }

  // A year and a bit of listening: evenings and weekends heavy, a few hundred favourites.
  const HOURS = [2, 1, 1, .5, .5, 1, 3, 6, 8, 6, 5, 5, 6, 7, 6, 5, 6, 8, 10, 12, 14, 16, 13, 7], HSUM = HOURS.reduce((a, b) => a + b);
  const favs = Array.from({ length: 400 }, () => pick(tracks)), plays = [];
  const addPlay = (t, date, ms) => plays.push({ t, utc: Math.floor(date / 1000), ms, hour: date.getHours(), dow: (date.getDay() + 6) % 7, month: date.getMonth() });
  for (let i = 0; i < 5400; i++) {
    const d = new Date();
    d.setDate(d.getDate() - Math.floor(420 * Math.pow(rnd(), 1.5)));
    if (d.getDay() % 6 && d.getDay() !== 5 && rnd() < .35) continue;
    let x = rnd() * HSUM, h = 0;
    while ((x -= HOURS[h]) > 0) h++;
    d.setHours(h, Math.floor(rnd() * 60));
    if (d > Date.now()) continue;
    const t = favs[Math.floor(400 * Math.pow(rnd(), 1.7))];
    addPlay(t, d, t.ms);
  }

  const page = (rows, a) => ({ total: rows.length, rows: rows.slice(a.offset || 0, (a.offset || 0) + (a.limit || 100)) });
  const has = (q, ...f) => { const s = f.join(' ').toLowerCase(); return q.toLowerCase().split(/\s+/).every(w => s.includes(w)); };
  const topN = (m, f) => [...m].sort((a, b) => b[1] - a[1]).slice(0, 8).map(([k, plays]) => ({ ...f(k), plays }));
  const bump = (m, k) => m.set(k, (m.get(k) || 0) + 1);
  let share = { active: false }, plSeq = 0, itemSeq = 0;
  const covers = new Map(), spec = t => `${t.codec}/${t.bits}/${t.rate}`;
  const playlists = [['Late drives', 14], ['Desk hours', 9]].map(([name, n]) => ({ id: ++plSeq, name, items: Array.from({ length: n }, () => ({ id: ++itemSeq, t: pick(favs) })) }));
  const plNamed = (name, not) => playlists.some(p => p.id !== not && p.name.toLowerCase() === name.toLowerCase());
  const plName = a => { const name = (a.name || '').trim(); if (!name) throw new Error('Give the playlist a name'); if (plNamed(name, a.id)) throw new Error(`A playlist called ${name} already exists`); return name; };

  const api = {
    summary: () => ({ tracks: tracks.length, albums: albums.length, artists: artists.length, ms: tracks.reduce((s, t) => s + t.ms, 0),
      bytes: tracks.reduce((s, t) => s + t.ms / 8 * t.kbps, 0), tiers: [0, 1, 2, 3].map(n => tracks.filter(t => t.tier === n).length) }),
    playlists: a => page(playlists.filter(p => !a.q || has(a.q, p.name)).sort((x, y) => x.name.localeCompare(y.name)).map(p => ({ id: p.id, name: p.name, n: p.items.length })), a),
    playlist_create: a => { const p = { id: ++plSeq, name: plName(a), items: [] }; playlists.push(p); return { id: p.id, name: p.name }; },
    playlist_rename: a => { const p = playlists.find(x => x.id === a.id); p.name = plName(a); return { id: p.id, name: p.name }; },
    playlist_delete: a => { playlists.splice(playlists.findIndex(x => x.id === a.id), 1); return null; },
    playlist_add: a => { const p = playlists.find(x => x.id === a.id), dup = p.items.some(i => i.t.id === a.track_id); if (!dup) p.items.push({ id: ++itemSeq, t: tracks[a.track_id - 1] }); return { added: !dup }; },
    playlist_remove: a => { const p = playlists.find(x => x.id === a.id); p.items = p.items.filter(i => i.id !== a.item); return null; },
    album: a => {
      const al = albums.find(x => x.id === a.album_id), mine = tracks.filter(t => t.album_id === a.album_id), years = mine.map(t => t.year), seen = new Set();
      return { id: al.id, title: al.title, artist: al.artist, n: mine.length, ms: mine.reduce((s, t) => s + t.ms, 0), size: mine.reduce((s, t) => s + t.size, 0),
        year: Math.max(...years), year_min: Math.min(...years), tier: Math.max(...mine.map(t => t.tier)),
        genres: [...new Set(mine.flatMap(t => (t.genres || artists[t.artist_id - 1].genres.map(g => GEN[g]).join(', ')).split(/,\s*/)))].sort().join(', '),
        formats: mine.filter(t => !seen.has(spec(t)) && seen.add(spec(t))).slice(0, 4).map(t => ({ codec: t.codec, bits: t.bits, rate: t.rate, kbps: t.kbps })) };
    },
    edit_track: a => { const t = edit(tracks[a.track_id - 1], a); reindex(); return { track: t, written: false }; },
    edit_album: a => {
      const mine = tracks.filter(t => t.album_id === a.album_id);
      for (const t of mine) edit(t, { album: a.title, album_artist: a.artist, year: a.year, genres: a.genres });
      const id = mine[0].album_id; if (a.year) albums.find(x => x.id === id).year = a.year;
      if (covers.has(a.album_id) && id !== a.album_id) { covers.set(id, covers.get(a.album_id)); covers.delete(a.album_id); }
      reindex(); return { album_id: id, tracks: mine.length, written: 0 };
    },
    tracks: a => a.playlist_id ? page(playlists.find(p => p.id === a.playlist_id).items.map(i => ({ ...i.t, item: i.id })), a) : a.album_id ? page(tracks.filter(t => t.album_id === a.album_id), a)
      : a.artist_id ? page(tracks.filter(t => t.artist_id === a.artist_id), a)
      : a.q ? page(sorted.title.filter(t => has(a.q, t.title, t.artist, t.album)), a)
      : page(sorted[a.sort] || sorted.title, a),
    albums: a => page(a.q ? albums.filter(x => has(a.q, x.title, x.artist)) : albums, a),
    artists: a => page((a.q ? artists.filter(x => has(a.q, x.name)) : artists).slice().sort((x, y) => x.name.localeCompare(y.name)), a),
    similar: a => {
      const src = tracks[a.track_id - 1], g = artists[src.artist_id - 1].genres;
      const seen = new Set(); // one track per artist, closest in year first
      return tracks.filter(t => t.artist_id !== src.artist_id && artists[t.artist_id - 1].genres.some(x => g.includes(x)))
        .sort((x, y) => Math.abs(x.year - src.year) - Math.abs(y.year - src.year) || (x.id * 7919 % 101) - (y.id * 7919 % 101))
        .filter(t => !seen.has(t.artist_id) && seen.add(t.artist_id)).slice(0, 12);
    },
    cover: a => ({ url: covers.get(a.album_id ?? tracks[a.track_id - 1].album_id) || null }),
    set_cover: a => { covers.set(a.album_id, 'data:image/jpeg;base64,' + a.data); return { tracks: tracks.filter(t => t.album_id === a.album_id).length, written: 0 }; },
    track_info: a => {
      const t = tracks[a.track_id - 1], mine = plays.filter(p => p.t === t), day = Math.floor(Date.now() / 1000) - (t.id * 7919 % 400) * 86400;
      return { track_no: t.no, disc_no: 1, added: day, modified: day - 86400 * 30, plays: mine.length, last_played: mine.length ? Math.max(...mine.map(p => p.utc)) : null,
        genres: t.genres || artists[t.artist_id - 1].genres.map(g => GEN[g]).join(', ') };
    },
    // Invented envelope and levels, shaped like a song: quiet intro, verses, louder choruses, fade.
    analyze: a => {
      let s = a.track_id * 2654435761 >>> 0;
      const r = () => (s = Math.imul(s ^ s >>> 15, 2246822507) + 0x9E3779B9 >>> 0) / 4294967296;
      const loud = .55 + r() * .45, parts = Array.from({ length: 8 }, () => .45 + r() * .55);
      const peaks = Array.from({ length: 240 }, (_, i) => {
        const x = i / 240, edge = Math.min(1, x * 14, (1 - x) * 9);
        return Math.round(255 * Math.min(1, loud * parts[Math.floor(x * 8)] * edge * (.62 + r() * .38)));
      });
      const peak_db = 20 * Math.log10(Math.max(...peaks) / 255), crest = 7 + r() * 9;
      return { peaks, peak_db, rms_db: peak_db - crest, dr: Math.round(crest - 1), clipped: peak_db > -.05 ? Math.floor(r() * 40) : 0 };
    },
    scan_start: () => { throw new Error('This preview uses an invented library. Scanning works in the installed app.'); },
    scan_status: () => ({ running: false, total: 0, done: 0, added: 0, removed: 0, errors: 0, message: '' }),
    log_play: a => { if (a.completed || a.ms_played >= 30000) addPlay(tracks[a.track_id - 1], new Date(a.started * 1000), a.ms_played); return null; },
    dash: a => {
      const inRange = plays.filter(p => p.utc >= (a.since || 0));
      const sel = inRange.filter(p => a.kind === 'track' ? p.t.id === a.id : a.kind === 'artist' ? p.t.artist_id === a.id
        : a.kind === 'genre' ? artists[p.t.artist_id - 1].genres.includes(a.id - 1) : true);
      const punch = Array(168).fill(0), by_month = Array(12).fill(0);
      for (const p of sel) { punch[p.dow * 24 + p.hour]++; by_month[p.month]++; }
      const tt = new Map(), ta = new Map(), tg = new Map();
      for (const p of inRange) { bump(tt, p.t); bump(ta, p.t.artist_id); for (const g of artists[p.t.artist_id - 1].genres) bump(tg, g); }
      return {
        plays: sel.length, ms: sel.reduce((s, p) => s + p.ms, 0), punch, by_month,
        by_hour: Array.from({ length: 24 }, (_, h) => [0, 1, 2, 3, 4, 5, 6].reduce((s, d) => s + punch[d * 24 + h], 0)),
        by_dow: Array.from({ length: 7 }, (_, d) => punch.slice(d * 24, d * 24 + 24).reduce((s, v) => s + v, 0)),
        top_tracks: topN(tt, t => ({ id: t.id, name: t.title, sub: t.artist })),
        top_artists: topN(ta, id => ({ id, name: artists[id - 1].name })),
        top_genres: topN(tg, g => ({ id: g + 1, name: GEN[g] })),
      };
    },
    genre_map: () => {
      const n = GEN.map(() => 0), pl = GEN.map(() => 0), links = new Map();
      for (const t of tracks) for (const g of artists[t.artist_id - 1].genres) n[g]++;
      for (const p of plays) for (const g of artists[p.t.artist_id - 1].genres) pl[g]++;
      for (const a of artists) if (a.genres[1] != null) bump(links, [Math.min(...a.genres) + 1, Math.max(...a.genres) + 1].join());
      return { genres: GEN.map((name, i) => ({ id: i + 1, name, tracks: n[i], plays: pl[i] })).filter(g => g.tracks).sort((a, b) => b.tracks - a.tracks),
        links: [...links].map(([k, w]) => [...k.split(',').map(Number), w]) };
    },
    share_start: a => (share = { active: true, url: 'http://192.168.1.24:51730/s/3f9ac41e07b25d8861c0a97de4f2b6a1', expires: Math.floor(Date.now() / 1000) + a.minutes * 60, guests: 0, track_id: a.track_id }),
    share_sync: () => api.share_status(),
    share_status: () => (share.active && share.expires < Date.now() / 1000 ? (share = { active: false }) : share),
    share_stop: () => (share = { active: false }),
  };
  return { call: (cmd, args) => new Promise((ok, no) => { try { ok(api[cmd](args || {})); } catch (e) { no(e); } }) };
})();
