//! Discord Rich Presence through the Discord app's local pipe
//! (`\\.\pipe\discord-ipc-N`). No login and no token: the Discord desktop app
//! shows whatever activity a local program hands it for a given application id.
//!
//! Wire format: 4-byte opcode + 4-byte length (little endian), then JSON.
//! Opcodes: 0 handshake, 1 frame, 2 close, 3 ping, 4 pong.

use serde_json::{json, Value};
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// What Starhowl wants Discord to show.
#[derive(Clone, PartialEq, Debug)]
pub struct Presence {
    pub app_id: String,
    /// Big title line (replaces the application's own name).
    pub title: String,
    /// 0 Playing, 2 Listening, 3 Watching, 5 Competing.
    pub kind: u8,
    pub details: String,
    pub state: Option<String>,
    /// Image link (https://...) or the name of an art asset uploaded to the app.
    pub large_image: String,
    /// Shown when hovering the big picture.
    pub large_text: String,
    /// Small picture in the corner of the big one (None = no small picture).
    pub small_image: Option<String>,
    /// Shown when hovering the small picture.
    pub small_text: String,
    pub button: Option<(String, String)>,
    /// Unix seconds the "elapsed" timer counts from.
    pub start: Option<u64>,
}

enum Msg {
    Set(Box<Option<Presence>>),
    Stop,
}

pub struct Discord {
    tx: Sender<Msg>,
    status: Arc<Mutex<String>>,
    thread: Option<JoinHandle<()>>,
}

impl Discord {
    pub fn start() -> Discord {
        let (tx, rx) = channel::<Msg>();
        let status = Arc::new(Mutex::new("Off".to_string()));
        let st = status.clone();
        let thread = std::thread::Builder::new()
            .name("starhowl-discord".into())
            .spawn(move || run(rx, st))
            .ok();
        Discord { tx, status, thread }
    }

    /// Show `p`, or clear the activity with `None`. Cheap to call often:
    /// changes are collected and sent once typing settles.
    pub fn set(&self, p: Option<Presence>) {
        let _ = self.tx.send(Msg::Set(Box::new(p)));
    }

    pub fn status(&self) -> String {
        self.status.lock().unwrap().clone()
    }

    /// Clears the activity and disconnects (waits at most ~1 s).
    pub fn stop(mut self) {
        let _ = self.tx.send(Msg::Stop);
        if let Some(t) = self.thread.take() {
            let started = Instant::now();
            while !t.is_finished() && started.elapsed() < Duration::from_millis(1000) {
                std::thread::sleep(Duration::from_millis(5));
            }
            if t.is_finished() {
                let _ = t.join();
            }
        }
    }
}

/// Discord allows 5 activity updates per 20 seconds.
struct RateLimit(VecDeque<Instant>);

impl RateLimit {
    fn wait_needed(&mut self) -> Option<Duration> {
        let window = Duration::from_secs(20);
        while self.0.front().is_some_and(|t| t.elapsed() >= window) {
            self.0.pop_front();
        }
        if self.0.len() < 5 {
            None
        } else {
            Some(window - self.0.front().unwrap().elapsed() + Duration::from_millis(50))
        }
    }
    fn record(&mut self) {
        self.0.push_back(Instant::now());
    }
}

struct Conn {
    pipe: File,
    app_id: String,
}

fn run(rx: std::sync::mpsc::Receiver<Msg>, status: Arc<Mutex<String>>) {
    let set_status = |s: String| *status.lock().unwrap() = s;
    let mut want: Option<Presence> = None;
    let mut changed_at: Option<Instant> = None;
    let mut shown: Option<Presence> = None;
    let mut conn: Option<Conn> = None;
    let mut next_connect = Instant::now();
    let mut limit = RateLimit(VecDeque::new());
    let mut nonce: u64 = 0;

    loop {
        // Collect the newest request; wait a little so typing doesn't spam Discord.
        let wait = if changed_at.is_some() { Duration::from_millis(250) } else { Duration::from_secs(5) };
        match rx.recv_timeout(wait) {
            Ok(Msg::Set(p)) => {
                let p = *p;
                if p != want {
                    want = p;
                    changed_at = Some(Instant::now());
                }
                continue;
            }
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if changed_at.is_some_and(|t| t.elapsed() < Duration::from_millis(800)) {
            continue;
        }
        changed_at = None;

        let Some(p) = want.clone() else {
            // Turned off: clear what we showed and let go of the pipe.
            if let Some(mut c) = conn.take() {
                let _ = set_activity(&mut c.pipe, None, &mut nonce);
                crate::log!("discord: cleared");
            }
            shown = None;
            set_status("Off".into());
            continue;
        };

        if conn.as_ref().is_some_and(|c| c.app_id != p.app_id) {
            conn = None; // application id changed: reconnect under the new one
            shown = None;
        }
        if conn.is_none() {
            if Instant::now() < next_connect {
                continue;
            }
            match connect(&p.app_id) {
                Ok((pipe, user)) => {
                    crate::log!("discord: connected as {user}");
                    set_status(format!("Connected to Discord as {user}"));
                    conn = Some(Conn { pipe, app_id: p.app_id.clone() });
                    shown = None;
                }
                Err(e) => {
                    set_status(e);
                    next_connect = Instant::now() + Duration::from_secs(15);
                    continue;
                }
            }
        }

        if shown.as_ref() == Some(&p) {
            continue;
        }
        if let Some(d) = limit.wait_needed() {
            // Try again once the window allows it.
            changed_at = Some(Instant::now() + d - Duration::from_millis(800));
            continue;
        }
        let c = conn.as_mut().unwrap();
        limit.record();
        match set_activity(&mut c.pipe, Some(&p), &mut nonce) {
            Ok(reply) => {
                if reply["evt"] == "ERROR" {
                    let msg = reply["data"]["message"].as_str().unwrap_or("unknown error").to_string();
                    crate::log!("discord: rejected: {reply}");
                    set_status(format!("Discord refused the update: {msg}"));
                    shown = Some(p); // don't retry the same bad data in a loop
                } else {
                    crate::log!("discord: activity set: {}", reply["data"]);
                    if !status.lock().unwrap().starts_with("Connected") {
                        set_status("Connected to Discord".into());
                    }
                    shown = Some(p);
                }
            }
            Err(e) => {
                crate::log!("discord: pipe error: {e}");
                set_status("Lost the connection to Discord. Retrying…".into());
                conn = None;
                shown = None;
                next_connect = Instant::now() + Duration::from_secs(5);
                changed_at = Some(Instant::now());
            }
        }
    }

    if let Some(mut c) = conn.take() {
        let _ = set_activity(&mut c.pipe, None, &mut nonce);
    }
}

fn write_frame(pipe: &mut File, op: u32, body: &Value) -> io::Result<()> {
    let data = serde_json::to_vec(body)?;
    let mut buf = Vec::with_capacity(8 + data.len());
    buf.extend_from_slice(&op.to_le_bytes());
    buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
    buf.extend_from_slice(&data);
    pipe.write_all(&buf)?;
    pipe.flush()
}

fn read_frame(pipe: &mut File) -> io::Result<(u32, Value)> {
    loop {
        let mut head = [0u8; 8];
        pipe.read_exact(&mut head)?;
        let op = u32::from_le_bytes(head[0..4].try_into().unwrap());
        let len = u32::from_le_bytes(head[4..8].try_into().unwrap()) as usize;
        if len > 1 << 20 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "oversized frame"));
        }
        let mut body = vec![0u8; len];
        pipe.read_exact(&mut body)?;
        let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        if op == 3 {
            write_frame(pipe, 4, &v)?; // answer pings, keep reading
            continue;
        }
        return Ok((op, v));
    }
}

/// Opens the first Discord pipe that accepts our application id.
fn connect(app_id: &str) -> Result<(File, String), String> {
    if app_id.trim().is_empty() || !app_id.trim().chars().all(|c| c.is_ascii_digit()) {
        return Err("The application ID must be a number.".into());
    }
    let mut found_pipe = false;
    for i in 0..10 {
        let Ok(mut pipe) = OpenOptions::new().read(true).write(true).open(format!(r"\\.\pipe\discord-ipc-{i}"))
        else {
            continue;
        };
        found_pipe = true;
        if write_frame(&mut pipe, 0, &json!({ "v": 1, "client_id": app_id.trim() })).is_err() {
            continue;
        }
        match read_frame(&mut pipe) {
            Ok((1, v)) if v["evt"] == "READY" => {
                let u = &v["data"]["user"];
                let name = u["global_name"].as_str().or(u["username"].as_str()).unwrap_or("you").to_string();
                return Ok((pipe, name));
            }
            Ok((_, v)) => {
                crate::log!("discord: handshake refused: {v}");
                let msg = v["message"].as_str().unwrap_or("handshake refused");
                return Err(format!("Discord said: {msg}"));
            }
            Err(_) => continue,
        }
    }
    Err(if found_pipe {
        "Couldn't talk to Discord. Retrying…".into()
    } else {
        "Discord isn't running. Waiting for it…".into()
    })
}

/// A full web link Discord accepts (buttons allow http and https).
pub fn is_link(s: &str) -> bool {
    let s = s.trim();
    let rest = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://"));
    rest.is_some_and(|r| r.contains('.') && !r.starts_with('.'))
        && s.len() <= 512
        && !s.chars().any(char::is_whitespace)
}

/// A picture Discord can show: an https link, or the name of art uploaded to
/// the Discord application (letters, digits, `_`, `-`).
pub fn is_picture(s: &str) -> bool {
    let s = s.trim();
    (s.starts_with("https://") && is_link(s))
        || (!s.is_empty() && s.len() <= 256 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
}

/// Discord drops text shorter than 2 characters and cuts it at 128.
pub const MIN_TEXT: usize = 2;
pub const MAX_TEXT: usize = 128;
/// Button labels: 1-32 characters.
pub const MAX_BUTTON: usize = 32;

/// Discord wants 2-128 characters in text fields.
fn text(s: &str) -> Option<String> {
    let t = s.trim();
    if t.chars().count() < MIN_TEXT {
        None
    } else {
        Some(t.chars().take(MAX_TEXT).collect())
    }
}

fn activity_json(p: &Presence) -> Value {
    let mut a = json!({ "type": p.kind, "instance": false });
    // Phones can't hover, so Discord shows the big/small hover texts as extra
    // lines there. Leave out any text that repeats one already shown, or the
    // same words appear twice.
    let mut seen: Vec<String> = Vec::new();
    let mut fresh = |t: Option<String>| -> Option<String> {
        let t = t?;
        if seen.iter().any(|s| s.eq_ignore_ascii_case(&t)) {
            return None;
        }
        seen.push(t.clone());
        Some(t)
    };
    if let Some(t) = fresh(text(&p.title)) {
        a["name"] = json!(t);
    }
    if let Some(d) = fresh(text(&p.details)) {
        a["details"] = json!(d);
    }
    if let Some(s) = fresh(p.state.as_deref().and_then(text)) {
        a["state"] = json!(s);
    }
    if let Some(start) = p.start {
        a["timestamps"] = json!({ "start": start });
    }
    let mut assets = json!({});
    let image = p.large_image.trim();
    if is_picture(image) {
        assets["large_image"] = json!(image);
        if let Some(t) = fresh(text(&p.large_text)) {
            assets["large_text"] = json!(t);
        }
    }
    if let Some(small) = p.small_image.as_deref().map(str::trim).filter(|s| is_picture(s)) {
        assets["small_image"] = json!(small);
        if let Some(t) = fresh(text(&p.small_text)) {
            assets["small_text"] = json!(t);
        }
    }
    if assets.as_object().is_some_and(|o| !o.is_empty()) {
        a["assets"] = assets;
    }
    if let Some((label, url)) = &p.button {
        let label: String = label.trim().chars().take(MAX_BUTTON).collect();
        if !label.is_empty() && is_link(url) {
            a["buttons"] = json!([{ "label": label, "url": url.trim() }]);
        }
    }
    a
}

fn set_activity(pipe: &mut File, p: Option<&Presence>, nonce: &mut u64) -> io::Result<Value> {
    *nonce += 1;
    let activity = p.map(activity_json).unwrap_or(Value::Null);
    let body = json!({
        "cmd": "SET_ACTIVITY",
        "args": { "pid": std::process::id(), "activity": activity },
        "nonce": nonce.to_string(),
    });
    write_frame(pipe, 1, &body)?;
    let (op, reply) = read_frame(pipe)?;
    if op == 2 {
        return Err(io::Error::new(io::ErrorKind::ConnectionAborted, reply.to_string()));
    }
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Presence {
        Presence {
            app_id: "1".into(),
            title: "Streaming With Starhowl".into(),
            kind: 0,
            details: "Capturing Xbox".into(),
            state: Some("Your text here".into()),
            large_image: "https://example.com/wolf.png".into(),
            large_text: "Big text".into(),
            small_image: Some("https://example.com/badge.png".into()),
            small_text: "Small text".into(),
            button: Some(("Watch".into(), "https://twitch.tv/someone".into())),
            start: Some(1_700_000_000),
        }
    }

    #[test]
    fn activity_has_every_field() {
        let a = activity_json(&sample());
        assert_eq!(a["name"], "Streaming With Starhowl");
        assert_eq!(a["details"], "Capturing Xbox");
        assert_eq!(a["state"], "Your text here");
        assert_eq!(a["assets"]["large_text"], "Big text");
        assert_eq!(a["assets"]["small_image"], "https://example.com/badge.png");
        assert_eq!(a["assets"]["small_text"], "Small text");
        assert_eq!(a["buttons"][0]["url"], "https://twitch.tv/someone");
        assert_eq!(a["timestamps"]["start"], 1_700_000_000);
    }

    #[test]
    fn short_or_bad_fields_are_left_out() {
        let mut p = sample();
        p.state = Some("x".into()); // Discord rejects 1-character text
        p.button = Some(("Watch".into(), "twitch.tv/no-scheme".into()));
        p.large_image = String::new();
        p.small_image = None;
        let a = activity_json(&p);
        assert!(a.get("state").is_none());
        assert!(a.get("buttons").is_none());
        assert!(a.get("assets").is_none());
    }

    #[test]
    fn bad_links_never_reach_discord() {
        let mut p = sample();
        p.large_image = "not a link".into();
        p.small_image = Some("http://insecure.example.com/x.png".into()); // pictures need https
        p.button = Some(("A very long button label that goes past the limit".into(), "https://ok.example.com".into()));
        let a = activity_json(&p);
        assert!(a.get("assets").is_none());
        assert_eq!(a["buttons"][0]["label"].as_str().unwrap().chars().count(), MAX_BUTTON);
    }

    #[test]
    fn repeated_text_is_sent_once() {
        // The mobile bug: middle text and big text both "Capturing Xbox One S".
        let mut p = sample();
        p.details = "Capturing Xbox One S".into();
        p.large_text = "capturing xbox one s".into();
        p.state = None;
        p.small_text = "Capturing Xbox One S".into();
        let a = activity_json(&p);
        assert_eq!(a["details"], "Capturing Xbox One S");
        assert!(a["assets"].get("large_text").is_none());
        assert!(a["assets"].get("small_text").is_none());
        // Different words still go through.
        p.large_text = "Xbox One S".into();
        assert_eq!(activity_json(&p)["assets"]["large_text"], "Xbox One S");
    }

    #[test]
    fn link_and_picture_rules() {
        assert!(is_link("https://twitch.tv/someone"));
        assert!(is_link("http://example.com"));
        assert!(!is_link("twitch.tv/someone"));
        assert!(!is_link("https://no spaces.com"));
        assert!(!is_link("https://localhost"));
        assert!(is_picture("https://example.com/a.png"));
        assert!(is_picture("starhowl"));
        assert!(!is_picture("http://example.com/a.png"));
        assert!(!is_picture("two words"));
    }
}
