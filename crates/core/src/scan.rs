//! Folder scanner. Incremental (unchanged files are skipped by mtime + size),
//! tags are parsed on all cores, and one writer batches rows into SQLite.
use anyhow::{anyhow, Result};
use lofty::config::ParseOptions;
use lofty::file::FileType;
use lofty::prelude::*;
use lofty::probe::Probe;
use rayon::prelude::*;
use rusqlite::{params, Connection, OptionalExtension, ToSql};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const EXTS: [&str; 13] = ["flac", "mp3", "m4a", "aac", "ogg", "opus", "wav", "aiff", "aif", "ape", "wv", "dsf", "dff"];

#[derive(Default)]
pub struct ScanState {
    pub running: AtomicBool,
    pub total: AtomicU64,
    pub done: AtomicU64,
    pub added: AtomicU64,
    pub removed: AtomicU64,
    pub errors: AtomicU64,
    pub message: Mutex<String>,
}

impl ScanState {
    pub fn json(&self) -> Value {
        json!({"running": self.running.load(Relaxed), "total": self.total.load(Relaxed), "done": self.done.load(Relaxed),
               "added": self.added.load(Relaxed), "removed": self.removed.load(Relaxed), "errors": self.errors.load(Relaxed),
               "message": *self.message.lock().unwrap()})
    }
}

pub(crate) struct Parsed {
    pub path: String, pub mtime: i64, pub size: i64, pub title: String, pub artist: String, pub album_artist: String, pub album: String,
    pub genres: Vec<String>, pub track_no: i64, pub disc_no: i64, pub year: Option<i64>, pub duration_ms: i64, pub codec: &'static str,
    pub sample_rate: i64, pub bit_depth: i64, pub bitrate: i64, pub channels: i64, pub tier: i64,
}

/// 3 = hi-res lossless, 2 = lossless, 1 = lossy at 256 kbps or more, 0 = lossy below that.
fn tier(lossless: bool, bit_depth: i64, sample_rate: i64, bitrate: i64) -> i64 {
    match (lossless, bit_depth > 16 || sample_rate > 48_000) {
        (true, true) => 3,
        (true, false) => 2,
        _ => (bitrate >= 256) as i64,
    }
}

pub(crate) fn parse(path: &Path, mtime: i64, size: i64) -> Result<Parsed> {
    if crate::dsd::is_dsd(&path.to_string_lossy()) {
        return crate::dsd::parse(path, mtime, size); // DSD has its own containers; the tag library doesn't read them
    }
    let file = Probe::open(path)?.options(ParseOptions::new().read_cover_art(false)).read()?;
    let p = file.properties();
    let bit_depth = p.bit_depth().unwrap_or(0) as i64;
    let (codec, lossless) = match file.file_type() {
        FileType::Flac => ("FLAC", true),
        FileType::Wav => ("WAV", true),
        FileType::Aiff => ("AIFF", true),
        FileType::Ape => ("APE", true),
        FileType::WavPack => ("WV", true),
        FileType::Mp4 if bit_depth > 0 => ("ALAC", true),
        FileType::Mp4 | FileType::Aac => ("AAC", false),
        FileType::Mpeg => ("MP3", false),
        FileType::Opus => ("OPUS", false),
        FileType::Vorbis => ("OGG", false),
        _ => ("AUDIO", false),
    };
    let tag = file.primary_tag().or_else(|| file.first_tag());
    let text = |k: ItemKey| tag.and_then(|t| t.get_string(&k)).map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    let stem = || path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let parent = || path.parent().and_then(Path::file_name).map(|s| s.to_string_lossy().into_owned());

    let artist = text(ItemKey::TrackArtist).unwrap_or_else(|| "Unknown Artist".into());
    let mut genres: Vec<String> = tag
        .map(|t| t.get_strings(&ItemKey::Genre).flat_map(|g| g.split([';', '\0'])).map(|g| g.trim().to_string()).filter(|g| !g.is_empty()).collect())
        .unwrap_or_default();
    genres.dedup();
    let sample_rate = p.sample_rate().unwrap_or(0) as i64;
    let bitrate = p.audio_bitrate().or(p.overall_bitrate()).unwrap_or(0) as i64;
    Ok(Parsed {
        path: path.to_string_lossy().into_owned(), mtime, size,
        title: text(ItemKey::TrackTitle).unwrap_or_else(stem),
        album_artist: text(ItemKey::AlbumArtist).unwrap_or_else(|| artist.clone()),
        album: text(ItemKey::AlbumTitle).or_else(parent).unwrap_or_else(|| "Unknown Album".into()),
        artist, genres,
        track_no: tag.and_then(|t| t.track()).unwrap_or(0) as i64,
        disc_no: tag.and_then(|t| t.disk()).unwrap_or(1) as i64,
        year: tag.and_then(|t| t.year()).map(|y| y as i64).filter(|y| *y > 0),
        duration_ms: p.duration().as_millis() as i64,
        codec, sample_rate, bit_depth, bitrate,
        channels: p.channels().unwrap_or(2) as i64,
        tier: tier(lossless, bit_depth, sample_rate, bitrate),
    })
}

/// Name -> row id caches. A scan preloads them; a single edit starts empty and falls back to lookups.
#[derive(Default)]
pub(crate) struct Ids {
    artists: HashMap<String, i64>,
    albums: HashMap<String, i64>,
    genres: HashMap<String, i64>,
}

fn id_for(db: &Connection, cache: &mut HashMap<String, i64>, key: String, find: &str, fp: &[&dyn ToSql], insert: &str, ip: &[&dyn ToSql]) -> Result<i64> {
    if let Some(id) = cache.get(&key) {
        return Ok(*id);
    }
    let found: Option<i64> = db.prepare_cached(find)?.query_row(fp, |r| r.get(0)).optional()?;
    let id = match found {
        Some(id) => id,
        None => {
            db.prepare_cached(insert)?.execute(ip)?;
            db.last_insert_rowid()
        }
    };
    cache.insert(key, id);
    Ok(id)
}

/// Writes one track into the index (insert, or update if the path is already known).
pub(crate) fn upsert(db: &Connection, t: &Parsed, now: i64, ids: &mut Ids, drop_analysis: bool) -> Result<i64> {
    const A: (&str, &str) = ("SELECT id FROM artists WHERE name=?1", "INSERT INTO artists(name) VALUES(?1)");
    let artist = id_for(db, &mut ids.artists, t.artist.to_lowercase(), A.0, &[&t.artist], A.1, &[&t.artist])?;
    let al_artist = id_for(db, &mut ids.artists, t.album_artist.to_lowercase(), A.0, &[&t.album_artist], A.1, &[&t.album_artist])?;
    let album = id_for(db, &mut ids.albums, format!("{}\u{1f}{al_artist}", t.album.to_lowercase()),
        "SELECT id FROM albums WHERE title=?1 AND artist_id=?2", &[&t.album, &al_artist],
        "INSERT INTO albums(title, artist_id, year) VALUES(?1,?2,?3)", &[&t.album, &al_artist, &t.year])?;
    let id: i64 = db.prepare_cached(
        "INSERT INTO tracks(path,mtime,size,title,artist_id,album_id,track_no,disc_no,year,duration_ms,codec,
                            sample_rate,bit_depth,bitrate,channels,tier,added_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)
         ON CONFLICT(path) DO UPDATE SET mtime=excluded.mtime, size=excluded.size, title=excluded.title,
           artist_id=excluded.artist_id, album_id=excluded.album_id, track_no=excluded.track_no, disc_no=excluded.disc_no,
           year=excluded.year, duration_ms=excluded.duration_ms, codec=excluded.codec, sample_rate=excluded.sample_rate,
           bit_depth=excluded.bit_depth, bitrate=excluded.bitrate, channels=excluded.channels, tier=excluded.tier
         RETURNING id")?
        .query_row(params![t.path, t.mtime, t.size, t.title, artist, album, t.track_no, t.disc_no, t.year, t.duration_ms,
                           t.codec, t.sample_rate, t.bit_depth, t.bitrate, t.channels, t.tier, now], |r| r.get(0))?;
    db.prepare_cached("DELETE FROM track_genres WHERE track_id=?1")?.execute([id])?;
    for g in &t.genres {
        let gid = id_for(db, &mut ids.genres, g.to_lowercase(), "SELECT id FROM genres WHERE name=?1", &[g], "INSERT INTO genres(name) VALUES(?1)", &[g])?;
        db.prepare_cached("INSERT OR IGNORE INTO track_genres VALUES(?1,?2)")?.execute([id, gid])?;
    }
    if drop_analysis {
        // The file itself changed: measurements and any artwork read from it are stale.
        db.prepare_cached("DELETE FROM analysis WHERE track_id=?1")?.execute([id])?;
        db.prepare_cached("DELETE FROM covers WHERE album_id=?1 AND custom=0")?.execute([album])?;
    }
    db.prepare_cached("DELETE FROM tracks_fts WHERE rowid=?1")?.execute([id])?;
    db.prepare_cached("INSERT INTO tracks_fts(rowid,title,artist,album) VALUES(?1,?2,?3,?4)")?
        .execute(params![id, t.title, t.artist, t.album])?;
    Ok(id)
}

/// Drops albums, artists and genres nothing points at any more and refreshes the stored counts.
pub(crate) const TIDY: &str = "
    DELETE FROM albums WHERE id NOT IN (SELECT album_id FROM tracks);
    UPDATE albums SET n_tracks=(SELECT COUNT(*) FROM tracks WHERE album_id=albums.id),
                      tier=(SELECT MAX(tier) FROM tracks WHERE album_id=albums.id),
                      year=(SELECT MAX(year) FROM tracks WHERE album_id=albums.id);
    DELETE FROM covers WHERE album_id NOT IN (SELECT id FROM albums);
    DELETE FROM artists WHERE id NOT IN (SELECT artist_id FROM tracks) AND id NOT IN (SELECT artist_id FROM albums);
    UPDATE artists SET n_tracks=(SELECT COUNT(*) FROM tracks WHERE artist_id=artists.id);
    DELETE FROM genres WHERE id NOT IN (SELECT genre_id FROM track_genres);";

/// Embeds a front cover in the file's tags, replacing any front cover already there.
pub(crate) fn embed_cover(path: &str, data: &[u8], png: bool) -> Result<()> {
    use lofty::config::WriteOptions;
    use lofty::picture::{MimeType, Picture, PictureType};
    use lofty::tag::Tag;
    let mut file = lofty::read_from_path(path)?;
    if file.primary_tag().is_none() {
        let kind = file.primary_tag_type();
        file.insert_tag(Tag::new(kind));
    }
    let tag = file.primary_tag_mut().ok_or_else(|| anyhow!("This file type can't hold tags"))?;
    tag.remove_picture_type(PictureType::CoverFront);
    tag.push_picture(Picture::new_unchecked(PictureType::CoverFront, Some(if png { MimeType::Png } else { MimeType::Jpeg }), None, data.to_vec()));
    file.save_to_path(path, WriteOptions::default())?;
    Ok(())
}

/// Writes edited tags into the audio file itself, so other players see the change too.
/// Only keys present in `a` are touched.
pub(crate) fn write_tags(path: &str, a: &serde_json::Value) -> Result<()> {
    use lofty::config::WriteOptions;
    use lofty::tag::Tag;
    let mut file = lofty::read_from_path(path)?;
    if file.primary_tag().is_none() {
        let kind = file.primary_tag_type();
        file.insert_tag(Tag::new(kind));
    }
    let tag = file.primary_tag_mut().ok_or_else(|| anyhow!("This file type can't hold tags"))?;
    let text = |k: &str| a[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    if let Some(v) = text("title") { tag.set_title(v); }
    if let Some(v) = text("artist") { tag.set_artist(v); }
    if let Some(v) = text("album") { tag.set_album(v); }
    if let Some(v) = text("album_artist") { tag.insert_text(ItemKey::AlbumArtist, v); }
    if let Some(v) = text("genres") { tag.set_genre(v.replace(',', ";")); }
    if let Some(v) = a["year"].as_u64() { tag.set_year(v as u32); }
    if let Some(v) = a["track_no"].as_u64().filter(|v| *v > 0) { tag.set_track(v as u32); }
    file.save_to_path(path, WriteOptions::default())?;
    Ok(())
}

fn load(db: &Connection, sql: &str) -> Result<HashMap<String, i64>> {
    let mut st = db.prepare(sql)?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn scan(db_path: &Path, root: &str, st: &ScanState) -> Result<()> {
    let root = std::fs::canonicalize(root).map_err(|e| anyhow!("Can't open folder {root}: {e}"))?;
    let db = crate::db::open(db_path)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;

    // What the index already holds under this folder.
    let mut prefix = root.to_string_lossy().into_owned();
    if !prefix.ends_with(std::path::MAIN_SEPARATOR) {
        prefix.push(std::path::MAIN_SEPARATOR);
    }
    let mut known: HashMap<String, (i64, i64, i64)> = HashMap::new();
    {
        let mut q = db.prepare("SELECT path, id, mtime, size FROM tracks WHERE substr(path,1,length(?1))=?1")?;
        for row in q.query_map([&prefix], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))? {
            let (p, v) = row?;
            known.insert(p, v);
        }
    }

    // Walk the tree; keep only new or changed files.
    *st.message.lock().unwrap() = "Looking for files".into();
    let mut todo: Vec<(PathBuf, i64, i64)> = Vec::new();
    let mut seen = 0u64;
    for e in walkdir::WalkDir::new(&root).follow_links(true).into_iter().filter_map(|e| e.ok()) {
        if !e.file_type().is_file() {
            continue;
        }
        let ext = e.path().extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
        if !EXTS.contains(&ext.as_str()) {
            continue;
        }
        let Ok(md) = e.metadata() else { continue };
        let mtime = md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
        let size = md.len() as i64;
        seen += 1;
        match known.remove(e.path().to_string_lossy().as_ref()) {
            Some((_, m, s)) if m == mtime && s == size => {}
            _ => todo.push((e.into_path(), mtime, size)),
        }
        st.total.store(seen, Relaxed);
    }
    let gone: Vec<i64> = known.values().map(|v| v.0).collect(); // still in `known` = no longer on disk
    st.total.store(todo.len() as u64, Relaxed);
    *st.message.lock().unwrap() = "Reading tags".into();

    let mut ids = Ids {
        artists: load(&db, "SELECT lower(name), id FROM artists")?,
        genres: load(&db, "SELECT lower(name), id FROM genres")?,
        albums: load(&db, "SELECT lower(title) || char(31) || artist_id, id FROM albums")?,
    };

    let (tx, rx) = std::sync::mpsc::sync_channel::<Result<Parsed>>(2048);
    std::thread::scope(|scope| -> Result<()> {
        scope.spawn(|| {
            todo.par_iter().for_each_with(tx, |tx, (p, m, s)| {
                let _ = tx.send(parse(p, *m, *s).map_err(|e| anyhow!("{}: {e}", p.display())));
            });
        });
        db.execute_batch("BEGIN")?;
        for (n, item) in rx.iter().enumerate() {
            st.done.fetch_add(1, Relaxed);
            let t = match item {
                Ok(t) => t,
                Err(e) => {
                    st.errors.fetch_add(1, Relaxed);
                    *st.message.lock().unwrap() = e.to_string();
                    continue;
                }
            };
            upsert(&db, &t, now, &mut ids, true)?;
            st.added.fetch_add(1, Relaxed);
            if n % 2000 == 1999 {
                db.execute_batch("COMMIT; BEGIN")?; // lets the UI see rows arrive during a long scan
            }
        }
        for id in &gone {
            db.prepare_cached("DELETE FROM tracks WHERE id=?1")?.execute([id])?;
            db.prepare_cached("DELETE FROM track_genres WHERE track_id=?1")?.execute([id])?;
            db.prepare_cached("DELETE FROM tracks_fts WHERE rowid=?1")?.execute([id])?;
            db.prepare_cached("DELETE FROM analysis WHERE track_id=?1")?.execute([id])?;
        }
        st.removed.store(gone.len() as u64, Relaxed);
        db.execute_batch(TIDY)?;
        db.execute_batch("DELETE FROM covers WHERE path IS NULL; COMMIT; PRAGMA optimize;")?;
        Ok(())
    })?;
    *st.message.lock().unwrap() = format!("Done. {seen} files in folder.");
    Ok(())
}
