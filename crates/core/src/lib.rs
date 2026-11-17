//! Sleeve core: library index, play history, stats and listen-together sessions.
//! Every front end (Tauri shell, dev server) talks to it through `Core::dispatch`.
pub mod analyze;
pub mod db;
pub mod dsd;
pub mod scan;
pub mod share;
pub mod stats;

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::SeqCst;
use std::sync::{Arc, Mutex};

pub struct Core {
    db_path: PathBuf,
    db: Mutex<Connection>,
    scan: Arc<scan::ScanState>,
    share: share::Share,
    media: std::sync::OnceLock<String>, // base URL of the in-app audio server
}

const COLS: &str = "t.id, t.title, a.name, al.title, t.duration_ms, t.codec, t.sample_rate, t.bit_depth, t.bitrate,
                    t.tier, t.path, t.year, t.artist_id, t.album_id, t.size, t.channels";
const FROM: &str = "FROM tracks t JOIN artists a ON a.id=t.artist_id JOIN albums al ON al.id=t.album_id";

fn track(r: &Row) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": r.get::<_, i64>(0)?, "title": r.get::<_, String>(1)?, "artist": r.get::<_, String>(2)?,
        "album": r.get::<_, String>(3)?, "ms": r.get::<_, i64>(4)?, "codec": r.get::<_, String>(5)?,
        "rate": r.get::<_, i64>(6)?, "bits": r.get::<_, i64>(7)?, "kbps": r.get::<_, i64>(8)?,
        "tier": r.get::<_, i64>(9)?, "path": r.get::<_, String>(10)?, "year": r.get::<_, Option<i64>>(11)?,
        "artist_id": r.get::<_, i64>(12)?, "album_id": r.get::<_, i64>(13)?,
        "size": r.get::<_, i64>(14)?, "ch": r.get::<_, i64>(15)?,
    }))
}

fn stamp() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

/// Turns what the user typed into a safe FTS5 prefix query: `pink flo` -> `"pink"* "flo"*`.
fn fts_query(q: Option<&str>) -> Option<String> {
    let terms: Vec<String> = q?
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\"*"))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

impl Core {
    pub fn open(db_path: &Path) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            db_path: db_path.to_path_buf(),
            db: Mutex::new(db::open(db_path)?),
            scan: Default::default(),
            share: Default::default(),
            media: Default::default(),
        }))
    }

    pub fn track_path(&self, id: i64) -> Result<String> {
        Ok(self.db.lock().unwrap().query_row("SELECT path FROM tracks WHERE id=?1", [id], |r| r.get(0))?)
    }

    fn covers_dir(&self) -> Result<PathBuf> {
        let dir = self.db_path.with_file_name("covers");
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    /// Looks for artwork in an album's own files: the first embedded picture, else a
    /// cover / folder / front / album image sitting in the same folder.
    fn find_cover(&self, album: i64) -> Result<Option<String>> {
        use lofty::file::TaggedFileExt;
        use lofty::picture::{MimeType, PictureType};
        let paths: Vec<String> = {
            let db = self.db.lock().unwrap();
            let mut st = db.prepare("SELECT path FROM tracks WHERE album_id=?1 ORDER BY disc_no, track_no LIMIT 50")?;
            let v = st.query_map([album], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            v
        };
        for path in &paths {
            if let Some((data, png)) = dsd::cover(path) {
                let out = self.covers_dir()?.join(format!("e{album}_{}.{}", stamp(), if png { "png" } else { "jpg" }));
                std::fs::write(&out, data)?;
                return Ok(Some(out.to_string_lossy().into_owned()));
            }
            let Ok(file) = lofty::read_from_path(path) else { continue };
            let pics: Vec<_> = file.tags().iter().flat_map(|t| t.pictures()).collect();
            if let Some(pic) = pics.iter().find(|p| p.pic_type() == PictureType::CoverFront).or(pics.first()) {
                let ext = match pic.mime_type() { Some(MimeType::Png) => "png", Some(MimeType::Gif) => "gif", Some(MimeType::Bmp) => "bmp", _ => "jpg" };
                let out = self.covers_dir()?.join(format!("e{album}_{}.{ext}", stamp()));
                std::fs::write(&out, pic.data())?;
                return Ok(Some(out.to_string_lossy().into_owned()));
            }
        }
        for e in paths.first().and_then(|p| Path::new(p).parent()).and_then(|d| std::fs::read_dir(d).ok()).into_iter().flatten().flatten() {
            let p = e.path();
            let stem = p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
            let ext = p.extension().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
            if ["cover", "folder", "front", "album"].contains(&stem.as_str()) && ["jpg", "jpeg", "png"].contains(&ext.as_str()) {
                return Ok(Some(p.to_string_lossy().into_owned()));
            }
        }
        Ok(None)
    }

    /// File path of an album's artwork (`thumb` = a 240 px square for lists). The answer is kept in
    /// the `covers` table, so each album's files are only searched once.
    pub fn album_cover(&self, album: i64, thumb: bool) -> Result<Option<String>> {
        let known: Option<(Option<String>, Option<String>)> = self.db.lock().unwrap()
            .query_row("SELECT path, thumb FROM covers WHERE album_id=?1", [album], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        let (path, small) = match known {
            Some(k) => k,
            None => {
                let found = self.find_cover(album)?;
                self.db.lock().unwrap().execute("INSERT OR REPLACE INTO covers(album_id, path, custom) VALUES(?1,?2,0)", params![album, found])?;
                (found, None)
            }
        };
        let Some(path) = path else { return Ok(None) };
        if !thumb {
            return Ok(Some(path));
        }
        if let Some(s) = small {
            return Ok(Some(s));
        }
        let out = self.covers_dir()?.join(format!("t{album}_{}.jpg", stamp()));
        let small = match image::open(&path) {
            Ok(img) => {
                img.resize_to_fill(240, 240, image::imageops::FilterType::Triangle).to_rgb8().save(&out)?;
                out.to_string_lossy().into_owned()
            }
            Err(_) => path, // a format we can't shrink: show the original
        };
        self.db.lock().unwrap().execute("UPDATE covers SET thumb=?1 WHERE album_id=?2", params![small, album])?;
        Ok(Some(small))
    }

    /// Sets an album's cover from image bytes: remembered by Sleeve, and embedded in every track that can be written.
    fn set_cover(&self, album: i64, data: &[u8]) -> Result<Value> {
        let png = data.starts_with(&[0x89, b'P', b'N', b'G']);
        if !png && !data.starts_with(&[0xFF, 0xD8]) {
            return Err(anyhow!("Use a JPEG or PNG image for the cover"));
        }
        let dir = self.covers_dir()?;
        let out = dir.join(format!("c{album}_{}.{}", stamp(), if png { "png" } else { "jpg" }));
        std::fs::write(&out, data)?;
        let tracks: Vec<(i64, String)> = {
            let db = self.db.lock().unwrap();
            let old: Option<(Option<String>, Option<String>)> =
                db.query_row("SELECT path, thumb FROM covers WHERE album_id=?1", [album], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
            for f in old.into_iter().flat_map(|o| [o.0, o.1]).flatten() {
                if Path::new(&f).starts_with(&dir) {
                    let _ = std::fs::remove_file(f); // only ever our own cached copies, never the user's files
                }
            }
            db.execute("INSERT OR REPLACE INTO covers(album_id, path, thumb, custom) VALUES(?1,?2,NULL,1)", params![album, out.to_string_lossy()])?;
            let mut st = db.prepare("SELECT id, path FROM tracks WHERE album_id=?1")?;
            let v = st.query_map([album], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
            v
        };
        let mut written = 0;
        for (id, path) in &tracks {
            if scan::embed_cover(path, data, png).is_ok() {
                written += 1;
                if let Ok(md) = std::fs::metadata(path) {
                    let mtime = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
                    self.db.lock().unwrap().execute("UPDATE tracks SET mtime=?1, size=?2 WHERE id=?3", params![mtime, md.len() as i64, id])?;
                }
            }
        }
        Ok(json!({"tracks": tracks.len(), "written": written}))
    }

    /// Applies edited details to one track: writes the file's tags, then re-indexes it.
    /// If the file can't be written (read-only storage), the index is still updated and `written` is false.
    fn edit_track(&self, id: i64, a: &Value, tidy: bool) -> Result<Value> {
        let path = self.track_path(id)?;
        let written = scan::write_tags(&path, a).is_ok();
        let md = std::fs::metadata(&path).map_err(|e| anyhow!("Can't find the file for this track: {e}"))?;
        let secs = |t: std::io::Result<std::time::SystemTime>| t.ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
        let mut t = scan::parse(Path::new(&path), secs(md.modified()), md.len() as i64)?;
        let text = |k: &str| a[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
        if let Some(v) = text("title") { t.title = v; }
        if let Some(v) = text("artist") { t.artist = v; }
        if let Some(v) = text("album") { t.album = v; }
        if let Some(v) = text("album_artist") { t.album_artist = v; }
        if let Some(v) = text("genres") { t.genres = v.split([',', ';']).map(|g| g.trim().to_string()).filter(|g| !g.is_empty()).collect(); }
        if let Some(v) = a["year"].as_i64() { t.year = Some(v).filter(|y| *y > 0); }
        if let Some(v) = a["track_no"].as_i64() { t.track_no = v; }
        let db = self.db.lock().unwrap();
        db.execute_batch("BEGIN")?;
        let done = scan::upsert(&db, &t, secs(Ok(std::time::SystemTime::now())), &mut Default::default(), false).and_then(|_| Ok(if tidy { db.execute_batch(scan::TIDY)? }));
        db.execute_batch(if done.is_ok() { "COMMIT" } else { "ROLLBACK" })?;
        done?;
        let row = db.query_row(&format!("SELECT {COLS} {FROM} WHERE t.id=?1"), [id], track)?;
        Ok(json!({"track": row, "written": written}))
    }

    /// Answers an HTTP request with a track's audio (DSD is converted to PCM as it streams).
    pub fn serve_track(&self, req: tiny_http::Request, id: i64) {
        match self.track_path(id) {
            Ok(p) => share::serve_audio(req, &p),
            Err(_) => {
                let _ = req.respond(tiny_http::Response::empty(404));
            }
        }
    }

    /// URL the app's own player can stream a track from. Only needed for formats the webview can't open as
    /// a plain file: it starts a tiny server bound to this device only, behind a random path.
    fn stream_url(self: &Arc<Self>, id: i64) -> Result<String> {
        if let Some(base) = self.media.get() {
            return Ok(format!("{base}{id}"));
        }
        let server = tiny_http::Server::http("127.0.0.1:0").map_err(|e| anyhow!("Can't start the audio server: {e}"))?;
        let port = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(0);
        let mut raw = [0u8; 12];
        getrandom::getrandom(&mut raw).map_err(|e| anyhow!("no random source: {e}"))?;
        let key: String = raw.iter().map(|b| format!("{b:02x}")).collect();
        let base = self.media.get_or_init(|| format!("http://127.0.0.1:{port}/{key}/")).clone();
        let me = self.clone();
        std::thread::spawn(move || {
            let prefix = format!("/{key}/");
            for req in server.incoming_requests() {
                let id = req.url().split('?').next().and_then(|u| u.strip_prefix(&prefix)).and_then(|s| s.parse().ok());
                let me = me.clone();
                std::thread::spawn(move || match id {
                    Some(id) => me.serve_track(req, id),
                    None => {
                        let _ = req.respond(tiny_http::Response::empty(404));
                    }
                });
            }
        });
        Ok(format!("{base}{id}"))
    }

    fn rows(&self, sql: &str, p: &[&dyn rusqlite::ToSql], f: fn(&Row) -> rusqlite::Result<Value>) -> Result<Vec<Value>> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare_cached(sql)?;
        let out = st.query_map(p, f)?.collect::<rusqlite::Result<_>>()?;
        Ok(out)
    }

    fn count(&self, sql: &str, p: &[&dyn rusqlite::ToSql]) -> Result<i64> {
        Ok(self.db.lock().unwrap().prepare_cached(sql)?.query_row(p, |r| r.get(0))?)
    }

    fn now_playing(&self, a: &Value) -> Result<share::Now> {
        let id = a["track_id"].as_i64().ok_or_else(|| anyhow!("track_id missing"))?;
        let (path, title, artist) = self.db.lock().unwrap().query_row(
            "SELECT t.path, t.title, a.name FROM tracks t JOIN artists a ON a.id=t.artist_id WHERE t.id=?1",
            [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(share::Now { track_id: id, path, title, artist, pos_ms: a["pos_ms"].as_u64().unwrap_or(0),
                        playing: a["playing"].as_bool().unwrap_or(true), at: None })
    }

    pub fn dispatch(self: &Arc<Self>, cmd: &str, a: &Value) -> Result<Value> {
        let int = |k: &str| a[k].as_i64();
        let limit = int("limit").unwrap_or(100).clamp(1, 500);
        let offset = int("offset").unwrap_or(0).max(0);
        let like = format!("%{}%", a["q"].as_str().unwrap_or("").trim());
        Ok(match cmd {
            "summary" => {
                let db = self.db.lock().unwrap();
                let (tracks, ms, bytes): (i64, i64, i64) = db.query_row(
                    "SELECT COUNT(*), COALESCE(SUM(duration_ms),0), COALESCE(SUM(size),0) FROM tracks", [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
                let mut tiers = [0i64; 4];
                let mut st = db.prepare_cached("SELECT tier, COUNT(*) FROM tracks GROUP BY tier")?;
                for row in st.query_map([], |r| Ok((r.get::<_, usize>(0)?, r.get::<_, i64>(1)?)))? {
                    let (t, c) = row?;
                    tiers[t.min(3)] = c;
                }
                json!({"tracks": tracks, "ms": ms, "bytes": bytes, "tiers": tiers,
                       "albums": db.query_row("SELECT COUNT(*) FROM albums", [], |r| r.get::<_, i64>(0))?,
                       "artists": db.query_row("SELECT COUNT(*) FROM artists WHERE n_tracks>0", [], |r| r.get::<_, i64>(0))?})
            }
            "tracks" => {
                if let Some(id) = int("playlist_id") {
                    // `item` identifies this entry in the playlist, so it can be removed.
                    let rows = self.rows(&format!("SELECT {COLS}, pt.id FROM playlist_tracks pt JOIN tracks t ON t.id=pt.track_id
                                                   JOIN artists a ON a.id=t.artist_id JOIN albums al ON al.id=t.album_id
                                                   WHERE pt.playlist_id=?1 ORDER BY pt.pos LIMIT ?2 OFFSET ?3"),
                                         params![id, limit, offset], |r| { let mut v = track(r)?; v["item"] = json!(r.get::<_, i64>(16)?); Ok(v) })?;
                    json!({"total": self.count("SELECT COUNT(*) FROM playlist_tracks pt JOIN tracks t ON t.id=pt.track_id WHERE pt.playlist_id=?1", params![id])?, "rows": rows})
                } else if let Some(id) = int("album_id") {
                    let rows = self.rows(&format!("SELECT {COLS} {FROM} WHERE t.album_id=?1 ORDER BY t.disc_no, t.track_no LIMIT ?2 OFFSET ?3"),
                                         params![id, limit, offset], track)?;
                    json!({"total": self.count("SELECT COUNT(*) FROM tracks WHERE album_id=?1", params![id])?, "rows": rows})
                } else if let Some(id) = int("artist_id") {
                    let rows = self.rows(&format!("SELECT {COLS} {FROM} WHERE t.artist_id=?1
                                                   ORDER BY al.year, al.title, t.disc_no, t.track_no LIMIT ?2 OFFSET ?3"),
                                         params![id, limit, offset], track)?;
                    json!({"total": self.count("SELECT COUNT(*) FROM tracks WHERE artist_id=?1", params![id])?, "rows": rows})
                } else if let Some(q) = fts_query(a["q"].as_str()) {
                    let rows = self.rows(&format!("SELECT {COLS} FROM tracks_fts JOIN tracks t ON t.id=tracks_fts.rowid
                                                   JOIN artists a ON a.id=t.artist_id JOIN albums al ON al.id=t.album_id
                                                   WHERE tracks_fts MATCH ?1 ORDER BY rank LIMIT ?2 OFFSET ?3"),
                                         params![q, limit, offset], track)?;
                    json!({"total": self.count("SELECT COUNT(*) FROM tracks_fts WHERE tracks_fts MATCH ?1", params![q])?, "rows": rows})
                } else {
                    // Each order walks an index, so paging stays fast however large the library gets.
                    let order = match a["sort"].as_str() {
                        Some("new") => "t.id DESC",
                        Some("quality") => "t.tier DESC, t.title COLLATE NOCASE",
                        _ => "t.title COLLATE NOCASE",
                    };
                    let rows = self.rows(&format!("SELECT {COLS} {FROM} ORDER BY {order} LIMIT ?1 OFFSET ?2"), params![limit, offset], track)?;
                    json!({"total": self.count("SELECT COUNT(*) FROM tracks", &[])?, "rows": rows})
                }
            }
            "albums" => {
                let rows = self.rows(
                    "SELECT al.id, al.title, a.name, al.year, al.n_tracks, al.tier FROM albums al JOIN artists a ON a.id=al.artist_id
                     WHERE al.title LIKE ?1 OR a.name LIKE ?1 ORDER BY al.title COLLATE NOCASE LIMIT ?2 OFFSET ?3",
                    params![like, limit, offset],
                    |r| Ok(json!({"id": r.get::<_, i64>(0)?, "title": r.get::<_, String>(1)?, "artist": r.get::<_, String>(2)?,
                                  "year": r.get::<_, Option<i64>>(3)?, "n": r.get::<_, i64>(4)?, "tier": r.get::<_, i64>(5)?})))?;
                json!({"rows": rows, "total": self.count(
                    "SELECT COUNT(*) FROM albums al JOIN artists a ON a.id=al.artist_id WHERE al.title LIKE ?1 OR a.name LIKE ?1", params![like])?})
            }
            "artists" => {
                let rows = self.rows(
                    "SELECT id, name, n_tracks FROM artists WHERE n_tracks>0 AND name LIKE ?1 ORDER BY name COLLATE NOCASE LIMIT ?2 OFFSET ?3",
                    params![like, limit, offset],
                    |r| Ok(json!({"id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?, "n": r.get::<_, i64>(2)?})))?;
                json!({"rows": rows, "total": self.count("SELECT COUNT(*) FROM artists WHERE n_tracks>0 AND name LIKE ?1", params![like])?})
            }
            // Metadata similarity: shared genres first, then closest release year. One track per other artist.
            "similar" => {
                let id = int("track_id").ok_or_else(|| anyhow!("track_id missing"))?;
                json!(self.rows(&format!(
                    "SELECT {COLS} FROM track_genres g1 JOIN track_genres g2 ON g2.genre_id=g1.genre_id
                     JOIN tracks t ON t.id=g2.track_id JOIN artists a ON a.id=t.artist_id JOIN albums al ON al.id=t.album_id
                     JOIN tracks src ON src.id=g1.track_id
                     WHERE g1.track_id=?1 AND t.artist_id!=src.artist_id
                     GROUP BY t.artist_id ORDER BY COUNT(DISTINCT g1.genre_id) DESC, abs(COALESCE(t.year,0)-COALESCE(src.year,0)), t.id*2654435761%1000 LIMIT 12"),
                    params![id], track)?)
            }
            "edit_track" => self.edit_track(int("track_id").ok_or_else(|| anyhow!("track_id missing"))?, a, true)?,
            // Renaming an album rewrites the album fields of every track on it.
            "edit_album" => {
                let id = int("album_id").ok_or_else(|| anyhow!("album_id missing"))?;
                let ids: Vec<i64> = { let db = self.db.lock().unwrap(); let mut st = db.prepare("SELECT id FROM tracks WHERE album_id=?1")?;
                    let v = st.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?; v };
                let cover: Option<(Option<String>, Option<String>, i64)> = self.db.lock().unwrap()
                    .query_row("SELECT path, thumb, custom FROM covers WHERE album_id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
                let change = json!({"album": a["title"], "album_artist": a["artist"], "year": a["year"], "genres": a["genres"]});
                let (mut written, mut album_id) = (0, id);
                for t in &ids {
                    let r = self.edit_track(*t, &change, false)?;
                    written += r["written"].as_bool().unwrap_or(false) as i64;
                    album_id = r["track"]["album_id"].as_i64().unwrap_or(id);
                }
                let db = self.db.lock().unwrap();
                if let Some((path, thumb, custom)) = cover.filter(|c| c.0.is_some() && album_id != id) {
                    // A new name means a new album row; its artwork follows it.
                    db.execute("INSERT OR REPLACE INTO covers(album_id, path, thumb, custom) VALUES(?1,?2,?3,?4)", params![album_id, path, thumb, custom])?;
                    db.execute("DELETE FROM covers WHERE album_id=?1", [id])?;
                }
                db.execute_batch(scan::TIDY)?;
                json!({"album_id": album_id, "tracks": ids.len(), "written": written})
            }
            // An album is whatever its tracks' tags say it is: this gathers the album page from them.
            "album" => {
                let id = int("album_id").unwrap_or(0);
                let db = self.db.lock().unwrap();
                let formats: Vec<Value> = db.prepare("SELECT DISTINCT codec, bit_depth, sample_rate, MAX(bitrate) FROM tracks WHERE album_id=?1 GROUP BY 1,2,3 ORDER BY 2 DESC, 3 DESC LIMIT 4")?
                    .query_map([id], |r| Ok(json!({"codec": r.get::<_, String>(0)?, "bits": r.get::<_, i64>(1)?, "rate": r.get::<_, i64>(2)?, "kbps": r.get::<_, i64>(3)?})))?
                    .collect::<rusqlite::Result<_>>()?;
                db.query_row(
                    "SELECT al.id, al.title, a.name, COUNT(t.id), COALESCE(SUM(t.duration_ms),0), COALESCE(SUM(t.size),0), MIN(t.year), MAX(t.year), MAX(t.tier),
                            (SELECT group_concat(name, ', ') FROM (SELECT DISTINCT g.name FROM tracks x JOIN track_genres tg ON tg.track_id=x.id
                                                                   JOIN genres g ON g.id=tg.genre_id WHERE x.album_id=al.id ORDER BY g.name))
                     FROM albums al JOIN artists a ON a.id=al.artist_id LEFT JOIN tracks t ON t.album_id=al.id WHERE al.id=?1 GROUP BY al.id", [id],
                    |r| Ok(json!({"id": r.get::<_, i64>(0)?, "title": r.get::<_, String>(1)?, "artist": r.get::<_, String>(2)?, "n": r.get::<_, i64>(3)?,
                                  "ms": r.get::<_, i64>(4)?, "size": r.get::<_, i64>(5)?, "year_min": r.get::<_, Option<i64>>(6)?, "year": r.get::<_, Option<i64>>(7)?,
                                  "tier": r.get::<_, i64>(8)?, "genres": r.get::<_, Option<String>>(9)?, "formats": formats})))?
            }
            "playlists" => {
                let rows = self.rows(
                    "SELECT p.id, p.name, (SELECT COUNT(*) FROM playlist_tracks pt JOIN tracks t ON t.id=pt.track_id WHERE pt.playlist_id=p.id)
                     FROM playlists p WHERE p.name LIKE ?1 ORDER BY p.name COLLATE NOCASE LIMIT ?2 OFFSET ?3", params![like, limit, offset],
                    |r| Ok(json!({"id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?, "n": r.get::<_, i64>(2)?})))?;
                json!({"rows": rows, "total": self.count("SELECT COUNT(*) FROM playlists WHERE name LIKE ?1", params![like])?})
            }
            "playlist_create" | "playlist_rename" => {
                let name = a["name"].as_str().map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| anyhow!("Give the playlist a name"))?;
                let db = self.db.lock().unwrap();
                let r = if cmd == "playlist_create" {
                    db.execute("INSERT INTO playlists(name, created) VALUES(?1, strftime('%s','now'))", [name])
                } else {
                    db.execute("UPDATE playlists SET name=?1 WHERE id=?2", params![name, int("id")])
                };
                r.map_err(|_| anyhow!("A playlist called {name} already exists"))?;
                json!({"id": if cmd == "playlist_create" { db.last_insert_rowid() } else { int("id").unwrap_or(0) }, "name": name})
            }
            "playlist_delete" => {
                let db = self.db.lock().unwrap();
                db.execute("DELETE FROM playlist_tracks WHERE playlist_id=?1", [int("id")])?;
                db.execute("DELETE FROM playlists WHERE id=?1", [int("id")])?;
                Value::Null
            }
            "playlist_add" => {
                let (pl, tr) = (int("id"), int("track_id"));
                let db = self.db.lock().unwrap();
                let has: i64 = db.query_row("SELECT COUNT(*) FROM playlist_tracks WHERE playlist_id=?1 AND track_id=?2", params![pl, tr], |r| r.get(0))?;
                if has == 0 {
                    db.execute("INSERT INTO playlist_tracks(playlist_id, pos, track_id)
                                VALUES(?1, (SELECT COALESCE(MAX(pos),0)+1 FROM playlist_tracks WHERE playlist_id=?1), ?2)", params![pl, tr])?;
                }
                json!({"added": has == 0})
            }
            "playlist_remove" => {
                self.db.lock().unwrap().execute("DELETE FROM playlist_tracks WHERE id=?1 AND playlist_id=?2", params![int("item"), int("id")])?;
                Value::Null
            }
            "stream_url" => json!({"url": self.stream_url(int("track_id").ok_or_else(|| anyhow!("track_id missing"))?)?}),
            "cover" => {
                let album = match int("album_id") {
                    Some(a) => a,
                    None => self.db.lock().unwrap().query_row("SELECT album_id FROM tracks WHERE id=?1", [int("track_id")], |r| r.get(0))?,
                };
                json!({"path": self.album_cover(album, a["thumb"].as_bool().unwrap_or(false))?})
            }
            "set_cover" => {
                use base64::Engine;
                let data = base64::engine::general_purpose::STANDARD.decode(a["data"].as_str().unwrap_or(""))
                    .map_err(|_| anyhow!("That image could not be read"))?;
                self.set_cover(int("album_id").ok_or_else(|| anyhow!("album_id missing"))?, &data)?
            }
            // Everything the now-playing screen shows beyond the list row.
            "track_info" => {
                let id = int("track_id").ok_or_else(|| anyhow!("track_id missing"))?;
                self.db.lock().unwrap().query_row(
                    "SELECT t.track_no, t.disc_no, t.added_at, t.mtime,
                            (SELECT COUNT(*) FROM plays WHERE track_id=t.id AND counted=1),
                            (SELECT MAX(started_utc) FROM plays WHERE track_id=t.id AND counted=1),
                            (SELECT group_concat(g.name, ', ') FROM track_genres tg JOIN genres g ON g.id=tg.genre_id WHERE tg.track_id=t.id)
                     FROM tracks t WHERE t.id=?1", [id],
                    |r| Ok(json!({"track_no": r.get::<_, i64>(0)?, "disc_no": r.get::<_, i64>(1)?, "added": r.get::<_, i64>(2)?,
                                  "modified": r.get::<_, i64>(3)?, "plays": r.get::<_, i64>(4)?,
                                  "last_played": r.get::<_, Option<i64>>(5)?, "genres": r.get::<_, Option<String>>(6)?})))?
            }
            // Decodes the file once (off the UI thread) and caches the result.
            "analyze" => {
                let id = int("track_id").ok_or_else(|| anyhow!("track_id missing"))?;
                let cached = self.db.lock().unwrap().query_row(
                    "SELECT peaks, peak_db, rms_db, dr, clipped FROM analysis WHERE track_id=?1", [id],
                    |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, f64>(1)?, r.get::<_, f64>(2)?, r.get::<_, Option<i64>>(3)?, r.get::<_, i64>(4)?))).optional()?;
                let (peaks, peak_db, rms_db, dr, clipped) = match cached {
                    Some(c) => c,
                    None => {
                        let a = analyze::analyze(&self.track_path(id)?, 240)?;
                        self.db.lock().unwrap().execute("INSERT OR REPLACE INTO analysis VALUES(?1,?2,?3,?4,?5,?6)",
                            params![id, a.peaks, a.peak_db, a.rms_db, a.dr, a.clipped])?;
                        (a.peaks, a.peak_db, a.rms_db, a.dr, a.clipped)
                    }
                };
                json!({"peaks": peaks, "peak_db": peak_db, "rms_db": rms_db, "dr": dr, "clipped": clipped})
            }
            "scan_start" => {
                let root = a["path"].as_str().ok_or_else(|| anyhow!("Choose a folder to scan"))?.to_string();
                if self.scan.running.swap(true, SeqCst) {
                    return Err(anyhow!("A scan is already running"));
                }
                for c in [&self.scan.total, &self.scan.done, &self.scan.added, &self.scan.removed, &self.scan.errors] {
                    c.store(0, SeqCst);
                }
                let me = self.clone();
                std::thread::spawn(move || {
                    if let Err(e) = scan::scan(&me.db_path, &root, &me.scan) {
                        *me.scan.message.lock().unwrap() = e.to_string();
                        me.scan.errors.fetch_add(1, SeqCst);
                    }
                    me.scan.running.store(false, SeqCst);
                });
                self.scan.json()
            }
            "scan_status" => self.scan.json(),
            "log_play" => {
                stats::log_play(&self.db.lock().unwrap(), int("track_id").ok_or_else(|| anyhow!("track_id missing"))?,
                    int("started").unwrap_or(0), int("offset_min").unwrap_or(0), int("ms_played").unwrap_or(0),
                    a["completed"].as_bool().unwrap_or(false))?;
                Value::Null
            }
            "dash" => stats::dash(&self.db.lock().unwrap(), int("since").unwrap_or(0), a["kind"].as_str(), int("id").unwrap_or(0))?,
            "genre_map" => stats::genre_map(&self.db.lock().unwrap())?,
            "share_start" => self.share.start(self.now_playing(a)?, a["minutes"].as_u64().unwrap_or(30))?,
            "share_sync" => {
                self.share.sync(self.now_playing(a)?);
                self.share.status()
            }
            "share_status" => self.share.status(),
            "share_stop" => {
                self.share.stop();
                self.share.status()
            }
            other => return Err(anyhow!("unknown command {other}")),
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fts() {
        assert_eq!(super::fts_query(Some(" pink  flo\"yd* ")).unwrap(), "\"pink\"* \"flo\"* \"yd\"*");
        assert!(super::fts_query(Some(" -- ")).is_none());
    }
}
