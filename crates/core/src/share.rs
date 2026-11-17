//! Listen-together session: a temporary, token-protected stream of whatever the host is
//! playing. Nothing is uploaded anywhere; the link dies when the timer runs out or the host ends it.
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::net::{IpAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tiny_http::{Header, Request, Response, Server, StatusCode};

pub fn header(k: &str, v: &str) -> Header {
    Header::from_bytes(k.as_bytes(), v.as_bytes()).unwrap()
}

pub fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "m4a" | "aac" => "audio/mp4",
        "ogg" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "aiff" | "aif" => "audio/aiff",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "application/octet-stream",
    }
}

fn parse_range(v: &str, size: u64) -> Option<(u64, u64)> {
    let (a, b) = v.trim().strip_prefix("bytes=")?.split_once('-')?;
    let last = size.checked_sub(1)?;
    let (s, e) = match (a.parse::<u64>().ok(), b.parse::<u64>().ok()) {
        (Some(s), Some(e)) => (s, e.min(last)),
        (Some(s), None) => (s, last),
        (None, Some(n)) => (size.saturating_sub(n), last),
        _ => return None,
    };
    (s <= e).then_some((s, e))
}

/// Streams anything readable and seekable with HTTP Range support, so players can seek without downloading everything.
pub fn serve_reader<R: Read + Seek + Send + 'static>(req: Request, mut src: R, size: u64, kind: &str) {
    let range = req.headers().iter().find(|h| h.field.equiv("Range")).and_then(|h| parse_range(h.value.as_str(), size));
    let (start, end) = range.unwrap_or((0, size.saturating_sub(1)));
    let len = if size == 0 { 0 } else { end - start + 1 };
    let _ = src.seek(SeekFrom::Start(start));
    let mut h = vec![header("Content-Type", kind), header("Accept-Ranges", "bytes"), header("Cache-Control", "no-store")];
    if range.is_some() {
        h.push(header("Content-Range", &format!("bytes {start}-{end}/{size}")));
    }
    let code = if range.is_some() { 206 } else { 200 };
    let _ = req.respond(Response::new(StatusCode(code), h, src.take(len), Some(len as usize), None));
}

pub fn serve_file(req: Request, path: &str) {
    match File::open(path) {
        Ok(f) => {
            let size = f.metadata().map(|m| m.len()).unwrap_or(0);
            serve_reader(req, f, size, mime(path));
        }
        Err(_) => {
            let _ = req.respond(Response::empty(404));
        }
    }
}

/// Audio as a browser can play it: the file itself, or for DSD a PCM conversion computed while streaming.
pub fn serve_audio(req: Request, path: &str) {
    if !crate::dsd::is_dsd(path) {
        return serve_file(req, path);
    }
    match crate::dsd::open_pcm(path) {
        Ok(pcm) => {
            let size = pcm.len();
            serve_reader(req, pcm, size, "audio/wav");
        }
        Err(e) => {
            let _ = req.respond(Response::from_string(e.to_string()).with_status_code(415));
        }
    }
}

#[derive(Clone, Default)]
pub struct Now {
    pub track_id: i64,
    pub path: String,
    pub title: String,
    pub artist: String,
    pub pos_ms: u64,
    pub playing: bool,
    pub at: Option<Instant>,
}

struct Session {
    url: String,
    expires: u64,
    stop: Arc<AtomicBool>,
    now: Arc<Mutex<Now>>,
    guests: Arc<Mutex<HashMap<IpAddr, Instant>>>,
}

#[derive(Default)]
pub struct Share(Mutex<Option<Session>>);

fn unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// This device's address on the local network (no packet is sent; the OS just picks a route).
fn lan_ip() -> String {
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("10.255.255.255:9").and_then(|_| s.local_addr()))
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".into())
}

impl Share {
    pub fn start(&self, now: Now, minutes: u64) -> Result<Value> {
        self.stop();
        let server = Server::http("0.0.0.0:0").map_err(|e| anyhow!("Can't open a port for the session: {e}"))?;
        let port = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(0);
        let mut raw = [0u8; 16];
        getrandom::getrandom(&mut raw).map_err(|e| anyhow!("no random source: {e}"))?;
        let token: String = raw.iter().map(|b| format!("{b:02x}")).collect();
        let s = Session {
            url: format!("http://{}:{port}/s/{token}", lan_ip()),
            expires: unix() + minutes.clamp(1, 240) * 60,
            stop: Arc::new(AtomicBool::new(false)),
            now: Arc::new(Mutex::new(Now { at: Some(Instant::now()), ..now })),
            guests: Default::default(),
        };
        let (stop, now, guests, expires) = (s.stop.clone(), s.now.clone(), s.guests.clone(), s.expires);
        std::thread::spawn(move || {
            let prefix = format!("/s/{token}");
            while !stop.load(Relaxed) && unix() < expires {
                if let Ok(Some(req)) = server.recv_timeout(Duration::from_millis(300)) {
                    let (prefix, now, guests) = (prefix.clone(), now.clone(), guests.clone());
                    std::thread::spawn(move || handle(req, &prefix, &now, &guests, expires));
                }
            }
            stop.store(true, Relaxed); // dropping `server` closes the port
        });
        *self.0.lock().unwrap() = Some(s);
        Ok(self.status())
    }

    /// The host app reports what it is playing; guests follow it.
    pub fn sync(&self, now: Now) {
        if let Some(s) = self.0.lock().unwrap().as_ref() {
            *s.now.lock().unwrap() = Now { at: Some(Instant::now()), ..now };
        }
    }

    pub fn status(&self) -> Value {
        let mut g = self.0.lock().unwrap();
        if g.as_ref().is_some_and(|s| s.stop.load(Relaxed) || unix() >= s.expires) {
            *g = None;
        }
        match g.as_ref() {
            None => json!({"active": false}),
            Some(s) => {
                let mut guests = s.guests.lock().unwrap();
                guests.retain(|_, seen| seen.elapsed() < Duration::from_secs(8));
                json!({"active": true, "url": s.url, "expires": s.expires, "guests": guests.len(),
                       "track_id": s.now.lock().unwrap().track_id})
            }
        }
    }

    pub fn stop(&self) {
        if let Some(s) = self.0.lock().unwrap().take() {
            s.stop.store(true, Relaxed);
        }
    }
}

fn handle(req: Request, prefix: &str, now: &Mutex<Now>, guests: &Mutex<HashMap<IpAddr, Instant>>, expires: u64) {
    let url = req.url().split('?').next().unwrap_or("").to_string();
    let Some(rest) = url.strip_prefix(prefix) else {
        let _ = req.respond(Response::empty(404));
        return;
    };
    let n = now.lock().unwrap().clone();
    match rest {
        "" | "/" => {
            let _ = req.respond(Response::from_string(GUEST_PAGE).with_header(header("Content-Type", "text/html; charset=utf-8")));
        }
        "/state" => {
            if let Some(ip) = req.remote_addr().map(|a| a.ip()) {
                guests.lock().unwrap().insert(ip, Instant::now());
            }
            let drift = if n.playing { n.at.map(|t| t.elapsed().as_millis() as u64).unwrap_or(0) } else { 0 };
            let body = json!({"track_id": n.track_id, "title": n.title, "artist": n.artist, "pos_ms": n.pos_ms + drift,
                              "playing": n.playing, "ends_in": expires.saturating_sub(unix())});
            let _ = req.respond(Response::from_string(body.to_string())
                .with_header(header("Content-Type", "application/json")).with_header(header("Cache-Control", "no-store")));
        }
        "/audio" => serve_audio(req, &n.path),
        _ => {
            let _ = req.respond(Response::empty(404));
        }
    }
}

const GUEST_PAGE: &str = r#"<!doctype html><html lang="en"><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1"><title>Sleeve session</title>
<style>
html,body{height:100%;margin:0}body{background:#111;color:#ededed;font:500 14px/1.3 "Helvetica Neue",Helvetica,Arial,Roboto,sans-serif;
display:flex;flex-direction:column;justify-content:space-between;padding:24px;box-sizing:border-box;text-transform:uppercase}
small{color:#8b8b8b;letter-spacing:.04em}h1{font-size:clamp(40px,13vw,96px);line-height:.88;letter-spacing:-.045em;margin:12px 0;
font-weight:700;overflow-wrap:anywhere}button{width:112px;height:112px;border-radius:50%;border:0;background:#ededed;color:#111;
font:700 14px inherit;letter-spacing:.04em;text-transform:uppercase}button:focus-visible{outline:2px solid #ededed;outline-offset:4px}
footer{display:flex;justify-content:space-between;align-items:flex-end}
</style>
<header><small>(Sleeve) Listening together</small><h1 id="t">Connecting</h1><div id="a"></div></header>
<footer><button id="b">Join</button><small id="e"></small></footer>
<audio id="au" preload="auto"></audio>
<script>
const $=i=>document.getElementById(i),au=$('au'),base=location.pathname.replace(/\/$/,'');let joined=false,cur=null,ended=false;
$('b').onclick=()=>{joined=!joined;$('b').textContent=joined?'Leave':'Join';if(!joined)au.pause();else tick()};
async function tick(){if(ended)return;let s;try{const r=await fetch(base+'/state',{cache:'no-store'});if(!r.ok)throw 0;s=await r.json()}
catch(e){ended=true;au.pause();$('t').textContent='Session ended';$('a').textContent='';$('e').textContent='';$('b').hidden=true;return}
$('t').textContent=s.title;$('a').textContent=s.artist;$('e').textContent='Ends in '+Math.ceil(s.ends_in/60)+' min';
if(!joined)return;if(cur!==s.track_id){cur=s.track_id;au.src=base+'/audio?t='+cur}
const want=s.pos_ms/1000;if(Math.abs(au.currentTime-want)>1.5)try{au.currentTime=want}catch(e){}
if(s.playing&&au.paused)au.play().catch(()=>{});if(!s.playing&&!au.paused)au.pause()}
tick();setInterval(tick,2000);
</script></html>"#;
