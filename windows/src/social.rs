//! Friends and chat, by @handle. A background thread keeps a WebSocket to the ULTRA relay
//! (server/), signs in with this computer's ULTRA ID key (no password), and reconnects by
//! itself. Everything else happens on the UI thread: the friend list, requests, and messages.
//!
//! Messages are end-to-end encrypted: each pair of friends shares a key derived from their two
//! ID keys (`identity::chat_key`), and every message is sealed with XChaCha20-Poly1305 bound to
//! who sent it to whom. The relay only ever sees ciphertext.
//!
//! The relay also hands out friends' public keys, so a dishonest relay (or anyone in the middle
//! of an unencrypted ws:// connection) could swap in its own. ULTRA therefore pins each friend's
//! key the first time it sees it (chat-keys.json) and refuses to send to, or read from, a friend
//! whose key later changes until the user confirms it; each chat shows a safety code (from both
//! keys) that two friends can compare in person. Chats are kept on this computer
//! (chats.json, the newest 500 messages per friend).

use std::{
    collections::HashMap,
    io::ErrorKind,
    net::{TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

use base64_lite as b64;
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use rand_core::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Digest;

/// The relay ULTRA signs in to unless settings name another: the owner's Raspberry Pi, reached
/// through a Cloudflare Tunnel over TLS. ULTRA never talks to a chat server unencrypted (plain
/// ws:// is allowed only to this computer itself, for testing).
pub const DEFAULT_SERVERS: &[&str] = &["wss://chat.oliverbar.net/ws"];
const AUTH_PREFIX: &[u8] = b"ultra-chat-auth-v1:";
const KEEP: usize = 500;
const CHATS: &str = "chats.json";
const PINS: &str = "chat-keys.json";

#[derive(Clone, Serialize, Deserialize)]
pub struct Msg {
    /// Who wrote it (a handle; yours for your own messages).
    pub from: String,
    pub text: String,
    pub ts: i64,
    pub id: String,
    /// Your own messages: false until the relay has it.
    #[serde(default)]
    pub sent: bool,
}

#[derive(Clone)]
pub struct Friend {
    pub handle: String,
    pub name: String,
    /// The key to use: the pinned one (first seen), never a different one the relay reports later.
    key: String,
    pub online: bool,
    /// The relay reported a key other than the pinned one: chat is paused until the user trusts it.
    changed: Option<String>,
}

/// The UI thread's side: connection state, friends, requests and chats.
pub struct Social {
    dir: PathBuf,
    out: Option<mpsc::Sender<String>>,
    pub status: String,
    pub error: String,
    pub me: Option<String>,
    pub friends: Vec<Friend>,
    /// Friend requests to you: (handle, display name).
    pub requests: Vec<(String, String)>,
    /// Requests you've sent and that are still waiting.
    pub sent: Vec<String>,
    /// Handles you've blocked (the relay keeps the list; this is its copy).
    pub blocked: Vec<String>,
    chats: HashMap<String, Vec<Msg>>,
    /// Friends' keys as first seen (handle -> public key, hex).
    pins: HashMap<String, String>,
    pub unread: HashMap<String, u32>,
    /// The conversation open in the panel.
    pub open: Option<String>,
    dirty: bool,
}

/// What the app should do after a server event.
pub enum Effect {
    None,
    /// Tell the user something (a message arrived, a request, a problem).
    Toast(String),
    /// The relay deleted this ULTRA ID: forget it here too.
    Deleted,
}

mod base64_lite {
    // Standard base64, enough for nonces and ciphertext (no extra dependency).
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    pub fn encode(b: &[u8]) -> String {
        let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
        for c in b.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for i in 0..4 {
                s.push(if i <= c.len() { A[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
            }
        }
        s
    }
    pub fn decode(s: &str) -> Option<Vec<u8>> {
        let v: Vec<u8> = s.bytes().filter(|&c| c != b'=').map(|c| A.iter().position(|&x| x == c).map(|p| p as u8)).collect::<Option<_>>()?;
        let mut out = Vec::with_capacity(v.len() * 3 / 4);
        for c in v.chunks(4) {
            let n = c.iter().enumerate().fold(0u32, |n, (i, &x)| n | (x as u32) << (18 - 6 * i));
            for i in 0..c.len().saturating_sub(1) {
                out.push((n >> (16 - 8 * i)) as u8);
            }
        }
        Some(out)
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn seal(key: &[u8; 32], from: &str, to: &str, text: &str) -> (String, String) {
    let mut nonce = [0u8; 24];
    rand_core::OsRng.fill_bytes(&mut nonce);
    let body = json!({ "text": text }).to_string();
    let aad = format!("{from}>{to}");
    let c = XChaCha20Poly1305::new(key.into())
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: body.as_bytes(), aad: aad.as_bytes() })
        .unwrap_or_default();
    (b64::encode(&nonce), b64::encode(&c))
}

fn open_sealed(key: &[u8; 32], from: &str, to: &str, n: &str, c: &str) -> Option<String> {
    let nonce = b64::decode(n)?;
    if nonce.len() != 24 {
        return None;
    }
    let aad = format!("{from}>{to}");
    let body = XChaCha20Poly1305::new(key.into()).decrypt(XNonce::from_slice(&nonce), Payload { msg: &b64::decode(c)?, aad: aad.as_bytes() }).ok()?;
    let v: Value = serde_json::from_slice(&body).ok()?;
    Some(v["text"].as_str()?.chars().take(4000).collect())
}

/// The connection thread: connect, sign in, pass every server frame to `deliver`, send what
/// the UI thread queues; on any failure wait a little and start over. Ends when the UI side
/// drops its sender.
fn run(dir: PathBuf, servers: Vec<String>, rx: mpsc::Receiver<String>, deliver: impl Fn(Value) + Send + 'static) {
    std::thread::spawn(move || {
        let mut backoff = 2;
        loop {
            deliver(json!({ "t": "_status", "s": "connecting" }));
            // Each server in turn; the first that answers is used until the connection drops.
            let mut last = String::new();
            for server in &servers {
                match session(&dir, server, &rx, &deliver) {
                    Ok(()) => return, // the app stopped chat
                    Err(e) => last = e,
                }
            }
            deliver(json!({ "t": "_status", "s": "offline", "m": last }));
            // Drain anything queued while offline (it would be stale), unless chat was stopped.
            loop {
                match rx.try_recv() {
                    Ok(_) => continue,
                    Err(mpsc::TryRecvError::Disconnected) => return,
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
            std::thread::sleep(Duration::from_secs(backoff));
            backoff = (backoff * 2).min(30);
        }
    });
}

fn session(dir: &Path, server: &str, rx: &mpsc::Receiver<String>, deliver: &impl Fn(Value)) -> Result<(), String> {
    let id = crate::identity::load(dir).ok_or("no ULTRA ID")?;
    let (rest, port) = match server.split_once("://") {
        Some(("wss", r)) => (r, 443),
        Some(("ws", r)) => (r, 80),
        _ => return Err("the chat server address must start with ws:// or wss://".into()),
    };
    let host = rest.split('/').next().unwrap_or("");
    let bare = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host).trim_matches(['[', ']']);
    if port == 80 && !matches!(bare, "127.0.0.1" | "localhost" | "::1") {
        return Err("chat servers must use an encrypted wss:// address".into());
    }
    let host = if host.contains(':') { host.to_string() } else { format!("{host}:{port}") };
    let host = host.as_str();
    // A name can resolve to several addresses (IPv6 and IPv4, like a Raspberry Pi's .local name):
    // try each, IPv4 first, since a server may listen on only one of them.
    let mut addrs: Vec<_> = host.to_socket_addrs().map_err(|_| format!("can't find {host}"))?.collect();
    addrs.sort_by_key(|a| a.is_ipv6());
    let tcp = addrs
        .iter()
        .find_map(|a| TcpStream::connect_timeout(a, Duration::from_secs(4)).ok())
        .ok_or_else(|| format!("can't reach {host}"))?;
    // TLS for wss:// (the system's own: SChannel on Windows, Secure Transport on a Mac).
    let (mut ws, _) = tungstenite::client_tls_with_config(server, tcp, None, None).map_err(|e| format!("couldn't connect securely to {host} ({e})"))?;
    match ws.get_ref() {
        tungstenite::stream::MaybeTlsStream::Plain(t) => t.set_read_timeout(Some(Duration::from_millis(150))).ok(),
        tungstenite::stream::MaybeTlsStream::NativeTls(t) => t.get_ref().set_read_timeout(Some(Duration::from_millis(150))).ok(),
        _ => None,
    };
    let mut last_ping = std::time::Instant::now();
    loop {
        match ws.read() {
            Ok(tungstenite::Message::Text(t)) => {
                let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                if v["t"] == "challenge" {
                    let nonce = b64::decode(v["nonce"].as_str().unwrap_or("")).unwrap_or_default();
                    let mut signed = AUTH_PREFIX.to_vec();
                    signed.extend(nonce);
                    let sig = crate::identity::sign(dir, &signed).ok_or("couldn't use your ULTRA ID key")?;
                    let hex: String = sig.iter().map(|b| format!("{b:02x}")).collect();
                    let auth = json!({ "t": "auth", "handle": id.handle, "name": id.name, "key": id.public, "sig": hex });
                    ws.send(tungstenite::Message::Text(auth.to_string())).map_err(|e| e.to_string())?;
                    continue;
                }
                deliver(v);
            }
            Ok(tungstenite::Message::Close(_)) => return Err("the server closed the connection".into()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => return Err(format!("connection lost ({e})")),
        }
        loop {
            match rx.try_recv() {
                Ok(text) => ws.send(tungstenite::Message::Text(text)).map_err(|e| e.to_string())?,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = ws.close(None);
                    return Ok(());
                }
            }
        }
        // Keep NATs and the server from dropping an idle connection.
        if last_ping.elapsed() > Duration::from_secs(25) {
            ws.send(tungstenite::Message::Ping(Vec::new())).map_err(|e| e.to_string())?;
            last_ping = std::time::Instant::now();
        }
    }
}

impl Social {
    pub fn new(dir: &Path) -> Social {
        let chats = std::fs::read_to_string(dir.join(CHATS)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Social {
            dir: dir.to_path_buf(),
            out: None,
            status: "off".into(),
            error: String::new(),
            me: None,
            friends: Vec::new(),
            requests: Vec::new(),
            sent: Vec::new(),
            blocked: Vec::new(),
            chats,
            pins: std::fs::read_to_string(dir.join(PINS)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
            unread: HashMap::new(),
            open: None,
            dirty: false,
        }
    }

    /// Connect (if there's an ULTRA ID): server events reach the app through `deliver`.
    pub fn start(&mut self, servers: Vec<String>, deliver: impl Fn(Value) + Send + 'static) {
        self.stop();
        if crate::identity::load(&self.dir).is_none() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.out = Some(tx);
        self.status = "connecting".into();
        run(self.dir.clone(), servers, rx, deliver);
    }

    pub fn stop(&mut self) {
        self.out = None; // the thread notices and closes the connection
        self.status = "off".into();
        self.me = None;
        self.friends.clear();
        self.requests.clear();
        self.sent.clear();
        self.blocked.clear();
    }

    fn send(&self, v: Value) {
        if let Some(tx) = &self.out {
            let _ = tx.send(v.to_string());
        }
    }

    /// A friend as the relay describes them, checked against the pinned key: first sight pins it,
    /// a different key later is held back (Friend::changed) instead of used.
    fn checked(&mut self, f: &Value) -> Friend {
        let handle: String = f["handle"].as_str().unwrap_or("").into();
        let reported: String = f["key"].as_str().unwrap_or("").into();
        let (key, changed) = match self.pins.get(&handle) {
            Some(pinned) if *pinned != reported => (pinned.clone(), Some(reported)),
            Some(pinned) => (pinned.clone(), None),
            None => {
                self.pins.insert(handle.clone(), reported.clone());
                self.save_pins();
                (reported, None)
            }
        };
        Friend { handle, name: f["name"].as_str().unwrap_or("").into(), key, online: f["online"].as_bool().unwrap_or(false), changed }
    }

    fn save_pins(&self) {
        if let Ok(s) = serde_json::to_string(&self.pins) {
            let _ = crate::library::write_atomic(&self.dir.join(PINS), s);
        }
    }

    /// The user compared the new key's safety code with their friend and confirmed it: pin that
    /// key, and only if it is still the one whose code they were shown (`code`).
    pub fn trust(&mut self, handle: &str, code: &str) -> bool {
        let Some(new) = self.friend(handle).and_then(|f| f.changed.clone()) else { return false };
        if code.is_empty() || self.safety_code(&new) != code {
            return false;
        }
        if let Some(f) = self.friends.iter_mut().find(|f| f.handle == handle) {
            f.changed = None;
            f.key = new.clone();
        }
        self.pins.insert(handle.to_string(), new);
        self.save_pins();
        true
    }

    /// A code two friends can read to each other to be sure nobody is in the middle: from both
    /// keys, the same on both sides.
    fn safety_code(&self, friend_key: &str) -> String {
        let Some(mine) = crate::identity::load(&self.dir).map(|i| i.public) else { return String::new() };
        let (a, b) = if mine < friend_key.to_string() { (mine, friend_key.to_string()) } else { (friend_key.to_string(), mine) };
        let h = sha2::Sha256::digest(format!("ultra-safety-v1:{a}:{b}").as_bytes());
        let digits: String = h.iter().take(10).map(|b| format!("{:02}", b % 100)).collect();
        digits.as_bytes().chunks(5).map(|c| String::from_utf8_lossy(c).to_string()).collect::<Vec<_>>().join(" ")
    }

    fn friend(&self, handle: &str) -> Option<&Friend> {
        self.friends.iter().find(|f| f.handle == handle)
    }

    pub fn add(&mut self, handle: &str) {
        let h = handle.trim().trim_start_matches('@').to_lowercase();
        if !h.is_empty() {
            self.send(json!({ "t": "add", "handle": h }));
        }
    }
    pub fn accept(&mut self, handle: &str) {
        self.send(json!({ "t": "accept", "handle": handle }));
    }
    pub fn decline(&mut self, handle: &str) {
        self.requests.retain(|(h, _)| h != handle);
        self.sent.retain(|h| h != handle);
        self.send(json!({ "t": "decline", "handle": handle }));
    }
    pub fn remove(&mut self, handle: &str) {
        self.send(json!({ "t": "remove", "handle": handle }));
    }
    /// Ask the relay to delete this ULTRA ID and everything it holds about it (handle, friends,
    /// requests, waiting messages). False when not signed in, so nothing could be asked.
    pub fn delete_account(&mut self) -> bool {
        let Some(me) = self.me.clone().filter(|_| self.status == "online") else { return false };
        self.send(json!({ "t": "delete", "confirm": me }));
        true
    }
    /// Forget every chat, pinned key and friend on this computer (after the ID is deleted).
    pub fn wipe(&mut self) {
        self.stop();
        self.chats.clear();
        self.pins.clear();
        self.unread.clear();
        self.open = None;
        self.dirty = false;
        let _ = std::fs::remove_file(self.dir.join(CHATS));
        let _ = std::fs::remove_file(self.dir.join(PINS));
    }
    /// Block someone: no friendship, requests or messages from them (they aren't told).
    pub fn block(&mut self, handle: &str) {
        let h = handle.trim().trim_start_matches('@').to_lowercase();
        if !h.is_empty() {
            self.send(json!({ "t": "block", "handle": h }));
        }
    }
    pub fn unblock(&mut self, handle: &str) {
        self.send(json!({ "t": "unblock", "handle": handle }));
    }
    /// Show a conversation (marks it read).
    pub fn open(&mut self, handle: Option<String>) {
        if let Some(h) = &handle {
            self.unread.remove(h);
        }
        self.open = handle;
    }

    /// Send a message to the open conversation.
    pub fn say(&mut self, text: &str) -> Result<(), String> {
        let text: String = text.trim().chars().take(4000).collect();
        let (Some(to), Some(me)) = (self.open.clone(), self.me.clone()) else { return Err("not connected".into()) };
        if text.is_empty() {
            return Ok(());
        }
        let f = self.friend(&to).ok_or("you can only message friends")?;
        if f.changed.is_some() {
            return Err(format!("@{to}'s ULTRA ID key changed · check the safety code with them before chatting"));
        }
        let key = f.key.clone();
        let key = crate::identity::chat_key(&self.dir, &key).ok_or("couldn't make the chat key")?;
        let (n, c) = seal(&key, &me, &to, &text);
        let mut idb = [0u8; 9];
        rand_core::OsRng.fill_bytes(&mut idb);
        let id: String = idb.iter().map(|b| format!("{b:02x}")).collect();
        self.send(json!({ "t": "send", "to": to, "id": id, "n": n, "c": c }));
        self.push(&to, Msg { from: me, text, ts: now(), id, sent: false });
        Ok(())
    }

    fn push(&mut self, with: &str, m: Msg) {
        let log = self.chats.entry(with.to_string()).or_default();
        if log.iter().any(|x| x.id == m.id && x.from == m.from) {
            return;
        }
        log.push(m);
        if log.len() > KEEP {
            let extra = log.len() - KEEP;
            log.drain(..extra);
        }
        self.dirty = true;
    }

    /// Save chats if anything changed (call now and then, not on every message).
    pub fn save(&mut self) {
        if std::mem::take(&mut self.dirty) {
            if let Ok(s) = serde_json::to_string(&self.chats) {
                let _ = crate::library::write_atomic(&self.dir.join(CHATS), s);
            }
        }
    }

    /// A frame from the server (or the connection thread's own status notes).
    pub fn on_event(&mut self, v: &Value) -> Effect {
        let h = |k: &str| v[k].as_str().unwrap_or("").to_string();
        match v["t"].as_str().unwrap_or("") {
            "_status" => {
                self.status = h("s");
                self.error = h("m");
                if self.status != "online" {
                    for f in &mut self.friends {
                        f.online = false;
                    }
                }
            }
            "welcome" => {
                self.status = "online".into();
                self.error.clear();
                self.me = Some(h("handle"));
                self.blocked = v["blocked"].as_array().into_iter().flatten().filter_map(|b| b.as_str().map(String::from)).collect();
                let list: Vec<Value> = v["friends"].as_array().cloned().unwrap_or_default();
                self.friends = list.iter().map(|f| self.checked(f)).collect();
                if let Some(f) = self.friends.iter().find(|f| f.changed.is_some()) {
                    let who = f.handle.clone();
                    self.requests = v["requests"].as_array().into_iter().flatten().map(|r| (r["handle"].as_str().unwrap_or("").into(), r["name"].as_str().unwrap_or("").into())).collect();
                    self.sent = v["sent"].as_array().into_iter().flatten().filter_map(|s| s.as_str().map(String::from)).collect();
                    return Effect::Toast(format!("@{who}'s ULTRA ID key changed · chat with them is paused until you check it"));
                }
                self.requests = v["requests"].as_array().into_iter().flatten().map(|r| (r["handle"].as_str().unwrap_or("").into(), r["name"].as_str().unwrap_or("").into())).collect();
                self.sent = v["sent"].as_array().into_iter().flatten().filter_map(|s| s.as_str().map(String::from)).collect();
            }
            "error" => {
                if v["auth"] == true {
                    self.status = "error".into();
                    self.error = h("m");
                }
                return Effect::Toast(h("m"));
            }
            "requested" => {
                let who = h("handle");
                if !self.sent.contains(&who) {
                    self.sent.push(who.clone());
                }
                return Effect::Toast(format!("friend request sent to @{who}"));
            }
            "request" => {
                let who = h("handle");
                if !self.requests.iter().any(|(x, _)| *x == who) {
                    self.requests.push((who.clone(), h("name")));
                }
                return Effect::Toast(format!("@{who} wants to be friends · open friends to accept"));
            }
            "friend" => {
                let f = self.checked(&v["friend"]);
                self.requests.retain(|(x, _)| *x != f.handle);
                self.sent.retain(|x| *x != f.handle);
                let msg = format!("you and @{} are friends now", f.handle);
                self.friends.retain(|x| x.handle != f.handle);
                self.friends.push(f);
                self.friends.sort_by(|a, b| a.handle.cmp(&b.handle));
                return Effect::Toast(msg);
            }
            "declined" => {
                let who = h("handle");
                self.requests.retain(|(x, _)| *x != who);
                self.sent.retain(|x| *x != who);
            }
            "removed" => {
                let who = h("handle");
                self.friends.retain(|x| x.handle != who);
                if self.open.as_deref() == Some(&who) {
                    self.open = None;
                }
            }
            "deleted" => return Effect::Deleted,
            "blocked" => {
                let who = h("handle");
                self.friends.retain(|x| x.handle != who);
                self.requests.retain(|(x, _)| *x != who);
                self.sent.retain(|x| *x != who);
                if self.open.as_deref() == Some(&who) {
                    self.open = None;
                }
                if !self.blocked.contains(&who) {
                    self.blocked.push(who.clone());
                }
                return Effect::Toast(format!("blocked @{who} · they can't add or message you"));
            }
            "unblocked" => {
                let who = h("handle");
                self.blocked.retain(|x| *x != who);
                return Effect::Toast(format!("unblocked @{who}"));
            }
            "presence" => {
                let who = h("handle");
                if let Some(f) = self.friends.iter_mut().find(|f| f.handle == who) {
                    f.online = v["online"].as_bool().unwrap_or(false);
                }
            }
            "sent" => {
                let (to, id) = (h("to"), h("id"));
                if let Some(m) = self.chats.get_mut(&to).and_then(|l| l.iter_mut().rev().find(|m| m.id == id)) {
                    m.sent = true;
                    self.dirty = true;
                }
            }
            "msg" => {
                let from = h("from");
                let me = self.me.clone().unwrap_or_default();
                // Delivered: the relay can forget it (even if it can't be read, it never will be).
                if let Some(sid) = v["sid"].as_i64() {
                    self.send(json!({ "t": "got", "ids": [sid] }));
                }
                let Some(key) = self.friend(&from).filter(|f| f.changed.is_none()).and_then(|f| crate::identity::chat_key(&self.dir, &f.key)) else {
                    return Effect::Toast(format!("a message from @{from} was held back: their key changed and isn't confirmed"));
                };
                let Some(text) = open_sealed(&key, &from, &me, v["n"].as_str().unwrap_or(""), v["c"].as_str().unwrap_or("")) else {
                    return Effect::Toast(format!("a message from @{from} couldn't be decrypted"));
                };
                let ts = v["ts"].as_i64().unwrap_or_else(now);
                self.push(&from, Msg { from: from.clone(), text: text.clone(), ts, id: h("id"), sent: true });
                if self.open.as_deref() != Some(&from) {
                    *self.unread.entry(from.clone()).or_default() += 1;
                    let preview: String = text.chars().take(60).collect();
                    return Effect::Toast(format!("@{from}: {preview}"));
                }
            }
            _ => {}
        }
        Effect::None
    }

    /// Everything the friends panel shows.
    pub fn state(&self) -> Value {
        let last = |h: &str| self.chats.get(h).and_then(|l| l.last()).map(|m| m.ts).unwrap_or(0);
        let friends: Vec<Value> = self
            .friends
            .iter()
            .map(|f| json!({ "handle": f.handle, "name": f.name, "online": f.online, "unread": self.unread.get(&f.handle).copied().unwrap_or(0), "last": last(&f.handle), "changed": f.changed.is_some() }))
            .collect();
        let msgs: Vec<&Msg> = self.open.as_ref().and_then(|h| self.chats.get(h)).map(|l| l.iter().rev().take(200).rev().collect()).unwrap_or_default();
        json!({
            "status": self.status,
            "error": self.error,
            "me": self.me,
            "friends": friends,
            "requests": self.requests.iter().map(|(h, n)| json!({ "handle": h, "name": n })).collect::<Vec<_>>(),
            "sent": self.sent,
            "blocked": self.blocked,
            "open": self.open,
            // The code for the key in use; while a key change waits, the code for the *new* key,
            // which is the one the user is about to verify and trust.
            "safety": self.open.as_ref().and_then(|h| self.friend(h)).map(|f| self.safety_code(f.changed.as_deref().unwrap_or(&f.key))).unwrap_or_default(),
            "msgs": msgs,
            "unread": self.unread.values().sum::<u32>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for n in 0..40 {
            let data: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(b64::decode(&b64::encode(&data)).unwrap(), data);
        }
        assert_eq!(b64::encode(b"hello"), "aGVsbG8=");
    }

    #[test]
    fn friend_keys_are_pinned_on_first_sight() {
        let dir = std::env::temp_dir().join(format!("ultra-pin-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut s = Social::new(&dir);
        let f = s.checked(&json!({ "handle": "sam", "name": "Sam", "key": "aa11", "online": true }));
        assert!(f.changed.is_none() && f.key == "aa11");
        // the relay later claims a different key: it's held back, the pinned one stays in use
        let f = s.checked(&json!({ "handle": "sam", "name": "Sam", "key": "bb22", "online": true }));
        assert_eq!((f.key.as_str(), f.changed.as_deref()), ("aa11", Some("bb22")));
        // trusting needs the new key's code: a wrong code (e.g. the old key's) pins nothing
        s.friends = vec![f];
        assert!(!s.trust("sam", "00000 00000 00000 00000"));
        assert_eq!(s.friends[0].key, "aa11");
        // and the pin survives a restart
        let mut s2 = Social::new(&dir);
        assert!(s2.checked(&json!({ "handle": "sam", "key": "bb22" })).changed.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sealed_messages_open_only_for_the_right_pair() {
        let key = [7u8; 32];
        let (n, c) = seal(&key, "olive", "sam", "hi sam");
        assert_eq!(open_sealed(&key, "olive", "sam", &n, &c).as_deref(), Some("hi sam"));
        assert!(open_sealed(&key, "eve", "sam", &n, &c).is_none(), "sender is bound in");
        assert!(open_sealed(&[8u8; 32], "olive", "sam", &n, &c).is_none(), "wrong key");
    }
}
