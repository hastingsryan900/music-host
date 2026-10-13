//! "Listen on your iPhone": a small web player served on the local network.
//! Off by default. Every request needs the secret token from the QR code,
//! so only devices you've shown it to can browse or play your library.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// What the server can hand out: the library as JSON, audio files by
/// track id, and covers by album id.
#[derive(Default)]
pub struct Catalog {
    pub json: String,
    pub tracks: HashMap<u64, PathBuf>,
    /// album id → (cached HD cover, a track to pull embedded art from)
    pub covers: HashMap<String, (PathBuf, PathBuf)>,
}

pub struct Server {
    pub url: String,
    stop: Arc<AtomicBool>,
    catalog: Arc<RwLock<Catalog>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn token() -> String {
    let path = crate::library::app_dir().join("stream-token");
    if let Ok(t) = std::fs::read_to_string(&path) {
        let t = t.trim().to_string();
        if t.len() >= 16 {
            return t;
        }
    }
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let t: String = (0..24).map(|_| char::from_digit(rng.gen_range(0..36), 36).unwrap()).collect();
    let _ = std::fs::write(&path, &t);
    t
}

/// This Mac's address on the local network.
fn local_ip() -> Option<std::net::IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.168.0.1:9").ok()?; // no packets are sent
    Some(s.local_addr().ok()?.ip())
}

impl Server {
    pub fn start(catalog: Catalog) -> Result<Server, String> {
        let ip = local_ip().ok_or("not connected to a network")?;
        let listener = (8737..8757).find_map(|p| TcpListener::bind(("0.0.0.0", p)).ok()).ok_or("no free port")?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let token = token();
        let stop = Arc::new(AtomicBool::new(false));
        let catalog = Arc::new(RwLock::new(catalog));
        let (s, c, t) = (stop.clone(), catalog.clone(), token.clone());
        std::thread::Builder::new()
            .name("stream".into())
            .spawn(move || {
                while !s.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((conn, _)) => {
                            let (c, t) = (c.clone(), t.clone());
                            std::thread::spawn(move || {
                                let _ = handle(conn, &c, &t);
                            });
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(100)),
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Server { url: format!("http://{ip}:{port}/?t={token}"), stop, catalog })
    }

    pub fn set_catalog(&self, c: Catalog) {
        if let Ok(mut w) = self.catalog.write() {
            *w = c;
        }
    }
}

fn respond(conn: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    write!(conn, "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n", body.len())?;
    conn.write_all(body)
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| v))
}

fn handle(mut conn: TcpStream, catalog: &RwLock<Catalog>, token: &str) -> std::io::Result<()> {
    conn.set_nonblocking(false)?;
    conn.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(conn.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
    let mut range = None;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.eq_ignore_ascii_case("range") {
                range = Some(v.trim().to_string());
            }
        }
    }
    if method != "GET" && method != "HEAD" {
        return respond(&mut conn, "405 Method Not Allowed", "text/plain", b"");
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if query_param(query, "t") != Some(token) {
        return respond(&mut conn, "403 Forbidden", "text/plain", b"Scan the QR code in Music Host to listen.");
    }
    if path == "/" {
        return respond(&mut conn, "200 OK", "text/html; charset=utf-8", PAGE.as_bytes());
    }
    if path == "/api/library" {
        let json = catalog.read().map(|c| c.json.clone()).unwrap_or_default();
        return respond(&mut conn, "200 OK", "application/json", json.as_bytes());
    }
    if let Some(id) = path.strip_prefix("/art/") {
        let paths = catalog.read().ok().and_then(|c| c.covers.get(id).cloned());
        let Some((cached, track)) = paths else {
            return respond(&mut conn, "404 Not Found", "text/plain", b"");
        };
        let small = query_param(query, "s").is_some();
        let bytes = if small { thumb(id, &cached, &track) } else { None }
            .or_else(|| std::fs::read(&cached).ok())
            .or_else(|| crate::library::find_artwork(&track));
        return match bytes {
            Some(b) => {
                let ctype = if b.starts_with(&[0x89, b'P']) { "image/png" } else { "image/jpeg" };
                respond(&mut conn, "200 OK", ctype, &b)
            }
            None => respond(&mut conn, "404 Not Found", "text/plain", b""),
        };
    }
    if let Some(id) = path.strip_prefix("/audio/").and_then(|s| s.parse::<u64>().ok()) {
        let Some(file) = catalog.read().ok().and_then(|c| c.tracks.get(&id).cloned()) else {
            return respond(&mut conn, "404 Not Found", "text/plain", b"");
        };
        return send_file(&mut conn, &file, range.as_deref(), method == "HEAD");
    }
    respond(&mut conn, "404 Not Found", "text/plain", b"")
}

/// A 400px cover for the grid (cached on disk), so the library loads fast.
fn thumb(id: &str, cached: &std::path::Path, track: &std::path::Path) -> Option<Vec<u8>> {
    if !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let path = crate::library::app_dir().join("artwork").join(format!("phone-{id}.jpg"));
    if let Ok(b) = std::fs::read(&path) {
        return Some(b);
    }
    let src = std::fs::read(cached).ok().or_else(|| crate::library::find_artwork(track))?;
    let img = image::load_from_memory(&src).ok()?.thumbnail(400, 400).to_rgb8();
    let mut out = std::io::Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).encode_image(&img).ok()?;
    let bytes = out.into_inner();
    let _ = std::fs::write(&path, &bytes);
    Some(bytes)
}

fn audio_type(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("mp3") => "audio/mpeg",
        Some("m4a") | Some("mp4") | Some("aac") | Some("alac") => "audio/mp4",
        Some("flac") => "audio/flac",
        Some("wav") => "audio/wav",
        Some("ogg") | Some("oga") => "audio/ogg",
        Some("opus") => "audio/ogg; codecs=opus",
        Some("webm") => "audio/webm",
        _ => "application/octet-stream",
    }
}

/// Serve a file, honouring Range requests (Safari needs them to play and seek).
fn send_file(conn: &mut TcpStream, path: &std::path::Path, range: Option<&str>, head: bool) -> std::io::Result<()> {
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return respond(conn, "404 Not Found", "text/plain", b""),
    };
    let len = f.metadata()?.len();
    let ctype = audio_type(path);
    let (start, end) = match range.and_then(|r| r.strip_prefix("bytes=")).and_then(|r| r.split_once('-')) {
        Some((a, b)) => {
            let (s, e) = match (a.trim().parse::<u64>().ok(), b.trim().parse::<u64>().ok()) {
                (Some(s), Some(e)) => (s, e.min(len.saturating_sub(1))),
                (Some(s), None) => (s, len.saturating_sub(1)),
                (None, Some(n)) => (len.saturating_sub(n), len.saturating_sub(1)),
                (None, None) => (0, len.saturating_sub(1)),
            };
            if s >= len || s > e {
                write!(conn, "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
                return Ok(());
            }
            write!(
                conn,
                "HTTP/1.1 206 Partial Content\r\nContent-Type: {ctype}\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {s}-{e}/{len}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                e - s + 1
            )?;
            (s, e)
        }
        None => {
            write!(conn, "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nAccept-Ranges: bytes\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n")?;
            (0, len.saturating_sub(1))
        }
    };
    if head || len == 0 {
        return Ok(());
    }
    f.seek(SeekFrom::Start(start))?;
    std::io::copy(&mut f.take(end - start + 1), conn)?;
    Ok(())
}

const PAGE: &str = include_str!("stream.html");

#[cfg(test)]
mod tests {
    use super::*;

    fn get(port_url: &str, path: &str, range: Option<&str>) -> (String, Vec<u8>) {
        let addr = port_url.trim_start_matches("http://").split('/').next().unwrap().to_string();
        let mut s = TcpStream::connect(addr).unwrap();
        let extra = range.map(|r| format!("Range: {r}\r\n")).unwrap_or_default();
        write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\n{extra}\r\n").unwrap();
        let mut out = vec![];
        s.read_to_end(&mut out).unwrap();
        let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        (String::from_utf8_lossy(&out[..split]).to_string(), out[split + 4..].to_vec())
    }

    #[test]
    fn serves_with_token_and_ranges() {
        let dir = std::env::temp_dir().join("mh-stream-test");
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var("MH_DATA_DIR", &dir);
        let file = dir.join("a.mp3");
        std::fs::write(&file, (0..=255u8).collect::<Vec<_>>()).unwrap();
        let mut c = Catalog { json: "{\"albums\":[]}".into(), ..Default::default() };
        c.tracks.insert(7, file);
        let srv = Server::start(c).unwrap();
        let tok = srv.url.split("t=").nth(1).unwrap().to_string();
        let (h, _) = get(&srv.url, "/api/library", None);
        assert!(h.starts_with("HTTP/1.1 403"), "{h}");
        let (h, b) = get(&srv.url, &format!("/api/library?t={tok}"), None);
        assert!(h.starts_with("HTTP/1.1 200"));
        assert_eq!(b, b"{\"albums\":[]}");
        let (h, b) = get(&srv.url, &format!("/audio/7?t={tok}"), Some("bytes=10-19"));
        assert!(h.starts_with("HTTP/1.1 206"), "{h}");
        assert!(h.contains("Content-Range: bytes 10-19/256"));
        assert_eq!(b, (10..20u8).collect::<Vec<_>>());
        let (h, b) = get(&srv.url, &format!("/audio/7?t={tok}"), None);
        assert!(h.contains("audio/mpeg"));
        assert_eq!(b.len(), 256);
        let (h, _) = get(&srv.url, &format!("/?t={tok}"), None);
        assert!(h.contains("text/html"));
    }
}
