//! SQLite index. One file, WAL mode, every list query backed by an index.
use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

const SCHEMA: &str = "
PRAGMA journal_mode=WAL;
PRAGMA synchronous=NORMAL;
PRAGMA temp_store=MEMORY;
PRAGMA cache_size=-32000;
PRAGMA mmap_size=268435456;

CREATE TABLE IF NOT EXISTS artists(
  id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE COLLATE NOCASE, n_tracks INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS albums(
  id INTEGER PRIMARY KEY, title TEXT NOT NULL COLLATE NOCASE, artist_id INTEGER NOT NULL, year INTEGER,
  n_tracks INTEGER NOT NULL DEFAULT 0, tier INTEGER NOT NULL DEFAULT 0, UNIQUE(title, artist_id));
CREATE TABLE IF NOT EXISTS genres(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE COLLATE NOCASE);
CREATE TABLE IF NOT EXISTS tracks(
  id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, mtime INTEGER NOT NULL, size INTEGER NOT NULL,
  title TEXT NOT NULL, artist_id INTEGER NOT NULL, album_id INTEGER NOT NULL,
  track_no INTEGER NOT NULL DEFAULT 0, disc_no INTEGER NOT NULL DEFAULT 1, year INTEGER,
  duration_ms INTEGER NOT NULL DEFAULT 0, codec TEXT NOT NULL, sample_rate INTEGER NOT NULL DEFAULT 0,
  bit_depth INTEGER NOT NULL DEFAULT 0, bitrate INTEGER NOT NULL DEFAULT 0, channels INTEGER NOT NULL DEFAULT 2,
  tier INTEGER NOT NULL DEFAULT 0, added_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS tracks_title  ON tracks(title COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS tracks_tier   ON tracks(tier DESC, title COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS tracks_album  ON tracks(album_id, disc_no, track_no);
CREATE INDEX IF NOT EXISTS tracks_artist ON tracks(artist_id);
CREATE INDEX IF NOT EXISTS albums_title  ON albums(title COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS albums_artist ON albums(artist_id);
CREATE TABLE IF NOT EXISTS track_genres(
  track_id INTEGER NOT NULL, genre_id INTEGER NOT NULL, PRIMARY KEY(track_id, genre_id)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS tg_genre ON track_genres(genre_id, track_id);
CREATE VIRTUAL TABLE IF NOT EXISTS tracks_fts USING fts5(
  title, artist, album, prefix='2 3', tokenize='unicode61 remove_diacritics 2');

-- Append-only play log. uid is globally unique so logs from two devices can be merged later.
-- hour / dow / month are the listener's LOCAL buckets, fixed at write time.
CREATE TABLE IF NOT EXISTS plays(
  id INTEGER PRIMARY KEY, uid TEXT NOT NULL UNIQUE, track_id INTEGER NOT NULL,
  started_utc INTEGER NOT NULL, offset_min INTEGER NOT NULL, ms_played INTEGER NOT NULL,
  completed INTEGER NOT NULL, counted INTEGER NOT NULL,
  hour INTEGER NOT NULL, dow INTEGER NOT NULL, month INTEGER NOT NULL);
-- Playlists are the user's own ordered lists; they survive rescans.
CREATE TABLE IF NOT EXISTS playlists(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE COLLATE NOCASE, created INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS playlist_tracks(id INTEGER PRIMARY KEY, playlist_id INTEGER NOT NULL, pos INTEGER NOT NULL, track_id INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS pt_order ON playlist_tracks(playlist_id, pos);

-- Album artwork, found once and remembered. path NULL = looked, found none. custom = chosen by the user.
CREATE TABLE IF NOT EXISTS covers(album_id INTEGER PRIMARY KEY, path TEXT, thumb TEXT, custom INTEGER NOT NULL DEFAULT 0);

-- Cached per-track measurements (waveform envelope, levels). Dropped when the file changes.
CREATE TABLE IF NOT EXISTS analysis(
  track_id INTEGER PRIMARY KEY, peaks BLOB NOT NULL, peak_db REAL NOT NULL, rms_db REAL NOT NULL,
  dr INTEGER, clipped INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS plays_time  ON plays(started_utc);
CREATE INDEX IF NOT EXISTS plays_track ON plays(track_id);
";

pub fn open(path: &Path) -> Result<Connection> {
    let c = Connection::open(path)?;
    c.busy_timeout(std::time::Duration::from_secs(10))?;
    c.execute_batch(SCHEMA)?;
    Ok(c)
}
