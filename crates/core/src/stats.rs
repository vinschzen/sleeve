//! Play log and dashboard rollups.
use anyhow::Result;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

/// Local (hour 0-23, weekday 0=Mon..6=Sun, month 1-12) for a UTC timestamp plus the
/// listener's UTC offset at that moment. Stored per play so travel and DST never skew history.
pub fn local_buckets(utc: i64, offset_min: i64) -> (i64, i64, i64) {
    let local = utc + offset_min * 60;
    let days = local.div_euclid(86_400);
    let hour = local.rem_euclid(86_400) / 3600;
    let dow = (days + 3).rem_euclid(7); // 1970-01-01 was a Thursday
    let doe = (days + 719_468).rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    (hour, dow, if mp < 10 { mp + 3 } else { mp - 9 })
}

pub fn log_play(db: &Connection, track_id: i64, started_utc: i64, offset_min: i64, ms_played: i64, completed: bool) -> Result<()> {
    let (hour, dow, month) = local_buckets(started_utc, offset_min);
    let mut r = [0u8; 8];
    getrandom::getrandom(&mut r).ok();
    let uid = format!("{started_utc:x}-{track_id:x}-{}", r.iter().map(|b| format!("{b:02x}")).collect::<String>());
    // Same rule most scrobblers use: 30 seconds, or the whole track.
    let counted = completed || ms_played >= 30_000;
    db.execute(
        "INSERT INTO plays(uid,track_id,started_utc,offset_min,ms_played,completed,counted,hour,dow,month)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![uid, track_id, started_utc, offset_min, ms_played, completed, counted, hour, dow, month],
    )?;
    Ok(())
}

fn top(db: &Connection, sql: &str, since: i64) -> Result<Vec<Value>> {
    let mut st = db.prepare_cached(sql)?;
    let rows = st.query_map([since], |r| {
        Ok(json!({"id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?,
                  "sub": r.get::<_, Option<String>>(2)?, "plays": r.get::<_, i64>(3)?}))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// `kind` narrows the time charts to one track, artist or genre. Top lists stay global.
pub fn dash(db: &Connection, since: i64, kind: Option<&str>, id: i64) -> Result<Value> {
    let f = match kind {
        Some("track") => "AND p.track_id=?2",
        Some("artist") => "AND p.track_id IN (SELECT id FROM tracks WHERE artist_id=?2)",
        Some("genre") => "AND p.track_id IN (SELECT track_id FROM track_genres WHERE genre_id=?2)",
        _ => "AND ?2=?2",
    };
    let w = format!("FROM plays p WHERE p.counted=1 AND p.started_utc>=?1 {f}");

    let (plays, ms): (i64, i64) =
        db.query_row(&format!("SELECT COUNT(*), COALESCE(SUM(ms_played),0) {w}"), [since, id], |r| Ok((r.get(0)?, r.get(1)?)))?;

    let mut punch = vec![0i64; 7 * 24]; // [weekday][hour]
    let mut st = db.prepare(&format!("SELECT dow, hour, COUNT(*) {w} GROUP BY dow, hour"))?;
    for row in st.query_map([since, id], |r| Ok((r.get::<_, usize>(0)?, r.get::<_, usize>(1)?, r.get::<_, i64>(2)?)))? {
        let (d, h, c) = row?;
        punch[d * 24 + h] = c;
    }
    let mut by_month = vec![0i64; 12];
    let mut st = db.prepare(&format!("SELECT month, COUNT(*) {w} GROUP BY month"))?;
    for row in st.query_map([since, id], |r| Ok((r.get::<_, usize>(0)?, r.get::<_, i64>(1)?)))? {
        let (m, c) = row?;
        by_month[m - 1] = c;
    }
    let by_hour: Vec<i64> = (0..24).map(|h| (0..7).map(|d| punch[d * 24 + h]).sum()).collect();
    let by_dow: Vec<i64> = (0..7).map(|d| punch[d * 24..d * 24 + 24].iter().sum()).collect();

    Ok(json!({
        "plays": plays, "ms": ms, "punch": punch, "by_hour": by_hour, "by_dow": by_dow, "by_month": by_month,
        "top_tracks": top(db, "SELECT t.id, t.title, a.name, COUNT(*) c FROM plays p JOIN tracks t ON t.id=p.track_id
            JOIN artists a ON a.id=t.artist_id WHERE p.counted=1 AND p.started_utc>=?1 GROUP BY t.id ORDER BY c DESC LIMIT 8", since)?,
        "top_artists": top(db, "SELECT a.id, a.name, NULL, COUNT(*) c FROM plays p JOIN tracks t ON t.id=p.track_id
            JOIN artists a ON a.id=t.artist_id WHERE p.counted=1 AND p.started_utc>=?1 GROUP BY a.id ORDER BY c DESC LIMIT 8", since)?,
        "top_genres": top(db, "SELECT g.id, g.name, NULL, COUNT(*) c FROM plays p JOIN track_genres tg ON tg.track_id=p.track_id
            JOIN genres g ON g.id=tg.genre_id WHERE p.counted=1 AND p.started_utc>=?1 GROUP BY g.id ORDER BY c DESC LIMIT 8", since)?,
    }))
}

/// Genres in the collection plus how often two genres share an artist.
/// The UI pulls linked genres together on the sphere.
pub fn genre_map(db: &Connection) -> Result<Value> {
    let mut st = db.prepare(
        "SELECT g.id, g.name, COUNT(*) n,
                (SELECT COUNT(*) FROM plays p JOIN track_genres x ON x.track_id=p.track_id
                  WHERE x.genre_id=g.id AND p.counted=1)
         FROM track_genres tg JOIN genres g ON g.id=tg.genre_id GROUP BY g.id ORDER BY n DESC LIMIT 64")?;
    let genres: Vec<Value> = st
        .query_map([], |r| Ok(json!({"id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?,
                                     "tracks": r.get::<_, i64>(2)?, "plays": r.get::<_, i64>(3)?})))?
        .collect::<rusqlite::Result<_>>()?;
    let mut st = db.prepare(
        "WITH ag AS (SELECT DISTINCT t.artist_id a, tg.genre_id g FROM tracks t JOIN track_genres tg ON tg.track_id=t.id)
         SELECT x.g, y.g, COUNT(*) c FROM ag x JOIN ag y ON x.a=y.a AND x.g<y.g GROUP BY 1,2 ORDER BY c DESC LIMIT 400")?;
    let links: Vec<Value> = st
        .query_map([], |r| Ok(json!([r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?])))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(json!({"genres": genres, "links": links}))
}

#[cfg(test)]
mod tests {
    use super::local_buckets;
    #[test]
    fn buckets() {
        // 2026-11-13 15:36:00 UTC = Fri 22:36 in Jakarta (UTC+7)
        assert_eq!(local_buckets(1_794_584_160, 420), (22, 4, 11));
        // 2024-02-29 23:30 UTC, UTC+1 -> Fri 1 Mar 00:30
        assert_eq!(local_buckets(1_709_249_400, 60), (0, 4, 3));
        // 1999-12-31 23:59:59 UTC, UTC-5 -> Fri 18:59
        assert_eq!(local_buckets(946_684_799, -300), (18, 4, 12));
    }
}
