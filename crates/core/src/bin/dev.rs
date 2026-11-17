//! Runs the real core behind the UI in an ordinary browser, for development without a Tauri build:
//!   cargo run -p sleeve-core --bin sleeve-dev -- sleeve.db ui 4717
use sleeve_core::{share, Core};
use tiny_http::{Method, Response, Server};

fn main() -> anyhow::Result<()> {
    let arg: Vec<String> = std::env::args().collect();
    let db = arg.get(1).map(String::as_str).unwrap_or("sleeve.db");
    let ui = arg.get(2).cloned().unwrap_or_else(|| "ui".into());
    let port = arg.get(3).map(String::as_str).unwrap_or("4717");
    let core = Core::open(std::path::Path::new(db))?;
    let server = Server::http(format!("127.0.0.1:{port}")).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("Sleeve dev server on http://127.0.0.1:{port}");
    for mut req in server.incoming_requests() {
        let (core, ui) = (core.clone(), ui.clone());
        std::thread::spawn(move || {
            let url = req.url().split('?').next().unwrap_or("/").to_string();
            if let (Method::Post, Some(cmd)) = (req.method().clone(), url.strip_prefix("/api/")) {
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                let args = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
                let (code, out) = match core.dispatch(cmd, &args) {
                    Ok(v) => (200, v.to_string()),
                    Err(e) => (400, serde_json::json!({"error": e.to_string()}).to_string()),
                };
                let _ = req.respond(Response::from_string(out).with_status_code(code).with_header(share::header("Content-Type", "application/json")));
            } else if let Some(id) = url.strip_prefix("/stream/").and_then(|s| s.parse().ok()) {
                core.serve_track(req, id);
            } else if let Some((thumb, id)) = url.strip_prefix("/acover/").map(|s| (false, s)).or(url.strip_prefix("/athumb/").map(|s| (true, s)))
                .and_then(|(t, s)| s.parse().ok().map(|id| (t, id))) {
                match core.album_cover(id, thumb) {
                    Ok(Some(p)) => share::serve_file(req, &p),
                    _ => { let _ = req.respond(Response::empty(404)); }
                }
            } else if url.contains("..") {
                let _ = req.respond(Response::empty(400));
            } else {
                let rel = if url == "/" { "index.html" } else { url.trim_start_matches('/') };
                let kind = match rel.rsplit('.').next() { Some("css") => "text/css", Some("js") => "text/javascript", _ => "text/html; charset=utf-8" };
                match std::fs::read(std::path::Path::new(&ui).join(rel)) {
                    Ok(b) => { let _ = req.respond(Response::from_data(b).with_header(share::header("Content-Type", kind))); }
                    Err(_) => { let _ = req.respond(Response::empty(404)); }
                }
            }
        });
    }
    Ok(())
}
