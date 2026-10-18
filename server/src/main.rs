//! ULTRA's friends and chat relay. ULTRAs connect over a WebSocket and sign in with their
//! ULTRA ID: the server sends a random challenge, the client signs it with its ed25519 key. The
//! first sign-in with a handle claims it for that key; after that only that key can use it.
//!
//! Everything goes by @handle: friend requests, friends, messages. Messages are end-to-end
//! encrypted by the two ULTRAs (X25519 from their ID keys, XChaCha20-Poly1305), so this server
//! only ever stores and passes on ciphertext, and only between friends. A message is kept until
//! the recipient's ULTRA confirms it arrived, then deleted. Nothing else about a chat is stored.
//!
//! Protocol (JSON text frames), client -> server:
//!   {"t":"auth","handle","name","key":hex,"sig":hex}        after {"t":"challenge","nonce"}
//!   {"t":"add","handle"} {"t":"accept","handle"} {"t":"decline","handle"} {"t":"remove","handle"}
//!   {"t":"send","to","id","n","c"}                              n = nonce, c = ciphertext (base64)
//!   {"t":"got","ids":[…]}                                      messages delivered: delete them
//! server -> client: challenge, welcome, error, request, friend, removed, presence, msg, sent.

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    http::HeaderMap,
    extract::{
        ws::{Message, WebSocket},
        ConnectInfo, State, WebSocketUpgrade,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use futures_util::{SinkExt, StreamExt};
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use tokio::sync::mpsc;

/// Largest frame a client may send (a message's ciphertext is well under this).
const MAX_FRAME: usize = 64 * 1024;
/// Largest message ciphertext, base64.
const MAX_MESSAGE: usize = 24 * 1024;
/// Frames a client may send per second (sustained).
const RATE: f64 = 20.0;
/// Undelivered messages kept per recipient.
const MAX_QUEUED: i64 = 2000;
/// New handles one internet address may claim per day (against handle squatting).
const SIGNUPS_PER_DAY: usize = 5;
/// Friend requests one person may have waiting at once (against request spam).
const MAX_PENDING: i64 = 30;
/// What a client signs with its ID key to sign in: this prefix, then the server's nonce.
const AUTH_PREFIX: &[u8] = b"ultra-chat-auth-v1:";

type Tx = mpsc::UnboundedSender<String>;

struct Hub {
    db: Mutex<Connection>,
    /// Signed-in connections per handle (one person can have several ULTRAs open).
    online: Mutex<HashMap<String, Vec<(u64, Tx)>>>,
    next: Mutex<u64>,
    /// Recent new-handle sign-ups per client address.
    signups: Mutex<HashMap<String, Vec<Instant>>>,
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Same rule as ULTRA's settings: 3–20 characters, a–z, 0–9 and _, starting with a letter.
fn valid_handle(h: &str) -> bool {
    (3..=20).contains(&h.len())
        && h.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && h.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn clean_name(n: &str) -> String {
    n.trim().chars().filter(|c| !c.is_control()).take(40).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    (s.len() % 2 == 0).then(|| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()).flatten()
}

fn open_db(path: &PathBuf) -> Connection {
    let db = Connection::open(path).expect("open database");
    db.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE IF NOT EXISTS users (handle TEXT PRIMARY KEY, name TEXT NOT NULL, key TEXT NOT NULL UNIQUE, created INTEGER NOT NULL, seen INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS friends (a TEXT NOT NULL, b TEXT NOT NULL, since INTEGER NOT NULL, PRIMARY KEY (a, b));
         CREATE TABLE IF NOT EXISTS requests (src TEXT NOT NULL, dst TEXT NOT NULL, ts INTEGER NOT NULL, PRIMARY KEY (src, dst));
         CREATE TABLE IF NOT EXISTS messages (id INTEGER PRIMARY KEY, src TEXT NOT NULL, dst TEXT NOT NULL, cid TEXT NOT NULL, n TEXT NOT NULL, c TEXT NOT NULL, ts INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS messages_dst ON messages (dst);",
    )
    .expect("create tables");
    db
}

impl Hub {
    fn send_to(&self, handle: &str, v: &Value) {
        let text = v.to_string();
        if let Some(conns) = self.online.lock().unwrap().get(handle) {
            for (_, tx) in conns {
                let _ = tx.send(text.clone());
            }
        }
    }
    fn is_online(&self, handle: &str) -> bool {
        self.online.lock().unwrap().get(handle).is_some_and(|c| !c.is_empty())
    }
    fn user(&self, handle: &str) -> Option<(String, String)> {
        let db = self.db.lock().unwrap();
        db.query_row("SELECT name, key FROM users WHERE handle = ?1", [handle], |r| Ok((r.get(0)?, r.get(1)?))).optional().ok().flatten()
    }
    fn are_friends(&self, a: &str, b: &str) -> bool {
        let db = self.db.lock().unwrap();
        db.query_row("SELECT 1 FROM friends WHERE a = ?1 AND b = ?2", [a, b], |_| Ok(())).optional().ok().flatten().is_some()
    }
    fn friend_json(&self, handle: &str) -> Value {
        let (name, key) = self.user(handle).unwrap_or_default();
        json!({ "handle": handle, "name": name, "key": key, "online": self.is_online(handle) })
    }
    fn friends_of(&self, handle: &str) -> Vec<String> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare("SELECT b FROM friends WHERE a = ?1 ORDER BY b").unwrap();
        st.query_map([handle], |r| r.get(0)).unwrap().flatten().collect()
    }
    fn requests(&self, handle: &str) -> (Vec<Value>, Vec<String>) {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare("SELECT r.src, u.name FROM requests r JOIN users u ON u.handle = r.src WHERE r.dst = ?1 ORDER BY r.ts").unwrap();
        let incoming = st.query_map([handle], |r| Ok(json!({ "handle": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)? }))).unwrap().flatten().collect();
        let mut st = db.prepare("SELECT dst FROM requests WHERE src = ?1 ORDER BY ts").unwrap();
        let outgoing = st.query_map([handle], |r| r.get(0)).unwrap().flatten().collect();
        (incoming, outgoing)
    }
    fn queued(&self, handle: &str) -> Vec<Value> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare("SELECT id, src, cid, n, c, ts FROM messages WHERE dst = ?1 ORDER BY id").unwrap();
        st.query_map([handle], |r| {
            Ok(json!({ "t": "msg", "sid": r.get::<_, i64>(0)?, "from": r.get::<_, String>(1)?, "id": r.get::<_, String>(2)?,
                       "n": r.get::<_, String>(3)?, "c": r.get::<_, String>(4)?, "ts": r.get::<_, i64>(5)? }))
        })
        .unwrap()
        .flatten()
        .collect()
    }
    /// Tell a handle's friends whether it's online.
    fn announce(&self, handle: &str, online: bool) {
        for f in self.friends_of(handle) {
            self.send_to(&f, &json!({ "t": "presence", "handle": handle, "online": online }));
        }
    }
}

/// Verify a sign-in: the key signs AUTH_PREFIX + nonce; a new handle is claimed for that key.
fn sign_in(hub: &Hub, v: &Value, nonce: &[u8], ip: &str) -> Result<(String, String), String> {
    let handle = v["handle"].as_str().unwrap_or("").trim_start_matches('@').to_lowercase();
    let name = clean_name(v["name"].as_str().unwrap_or(""));
    let key_hex = v["key"].as_str().unwrap_or("").to_lowercase();
    if !valid_handle(&handle) {
        return Err("handles are 3–20 letters, numbers or _, starting with a letter".into());
    }
    let key: [u8; 32] = unhex(&key_hex).and_then(|k| k.try_into().ok()).ok_or("bad key")?;
    let sig: [u8; 64] = unhex(v["sig"].as_str().unwrap_or("")).and_then(|s| s.try_into().ok()).ok_or("bad signature")?;
    let vk = VerifyingKey::from_bytes(&key).map_err(|_| "bad key")?;
    let mut signed = AUTH_PREFIX.to_vec();
    signed.extend_from_slice(nonce);
    vk.verify(&signed, &Signature::from_bytes(&sig)).map_err(|_| "signature doesn't match")?;
    let db = hub.db.lock().unwrap();
    let owner: Option<String> = db.query_row("SELECT key FROM users WHERE handle = ?1", [&handle], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
    match owner {
        Some(k) if k != key_hex => return Err(format!("@{handle} is taken")),
        Some(_) => {
            db.execute("UPDATE users SET name = ?1, seen = ?2 WHERE handle = ?3", params![name, now(), handle]).map_err(|e| e.to_string())?;
        }
        None => {
            // One handle per ULTRA ID.
            let other: Option<String> = db.query_row("SELECT handle FROM users WHERE key = ?1", [&key_hex], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
            if let Some(o) = other {
                return Err(format!("this ULTRA ID is already @{o}"));
            }
            {
                let mut ups = hub.signups.lock().unwrap();
                let list = ups.entry(ip.to_string()).or_default();
                list.retain(|t| t.elapsed() < Duration::from_secs(24 * 3600));
                if list.len() >= SIGNUPS_PER_DAY {
                    return Err("too many new handles from this connection today · try again tomorrow".into());
                }
                list.push(Instant::now());
            }
            db.execute("INSERT INTO users (handle, name, key, created, seen) VALUES (?1, ?2, ?3, ?4, ?4)", params![handle, name, key_hex, now()]).map_err(|e| e.to_string())?;
        }
    }
    Ok((handle, name))
}

/// One request from a signed-in client. Returns a reply for that client, if any.
fn handle_msg(hub: &Hub, me: &str, v: &Value) -> Option<Value> {
    let other = v["handle"].as_str().unwrap_or("").trim_start_matches('@').to_lowercase();
    let err = |m: &str| Some(json!({ "t": "error", "m": m }));
    match v["t"].as_str().unwrap_or("") {
        "add" => {
            if other == me {
                return err("that's you");
            }
            let Some((name, _)) = hub.user(&other) else { return err(&format!("there's no @{other} yet")) };
            if hub.are_friends(me, &other) {
                return err(&format!("you and @{other} are already friends"));
            }
            let db = hub.db.lock().unwrap();
            // They already asked us: that's a yes from both.
            let theirs = db.execute("DELETE FROM requests WHERE src = ?1 AND dst = ?2", [&other, me]).unwrap_or(0) > 0;
            if theirs {
                let t = now();
                let _ = db.execute("INSERT OR IGNORE INTO friends VALUES (?1, ?2, ?3)", params![me, other, t]);
                let _ = db.execute("INSERT OR IGNORE INTO friends VALUES (?1, ?2, ?3)", params![other, me, t]);
                drop(db);
                hub.send_to(&other, &json!({ "t": "friend", "friend": hub.friend_json(me) }));
                return Some(json!({ "t": "friend", "friend": hub.friend_json(&other) }));
            }
            let pending: i64 = db.query_row("SELECT COUNT(*) FROM requests WHERE src = ?1", [me], |r| r.get(0)).unwrap_or(0);
            if pending >= MAX_PENDING {
                return err("you have too many friend requests waiting · cancel some first");
            }
            let _ = db.execute("INSERT OR IGNORE INTO requests VALUES (?1, ?2, ?3)", params![me, other, now()]);
            let my_name: String = db.query_row("SELECT name FROM users WHERE handle = ?1", [me], |r| r.get(0)).unwrap_or_default();
            drop(db);
            hub.send_to(&other, &json!({ "t": "request", "handle": me, "name": my_name }));
            Some(json!({ "t": "requested", "handle": other, "name": name }))
        }
        "accept" => {
            let db = hub.db.lock().unwrap();
            if db.execute("DELETE FROM requests WHERE src = ?1 AND dst = ?2", [&other, me]).unwrap_or(0) == 0 {
                return err(&format!("@{other} hasn't asked to be your friend"));
            }
            let t = now();
            let _ = db.execute("INSERT OR IGNORE INTO friends VALUES (?1, ?2, ?3)", params![me, other, t]);
            let _ = db.execute("INSERT OR IGNORE INTO friends VALUES (?1, ?2, ?3)", params![other, me, t]);
            drop(db);
            hub.send_to(&other, &json!({ "t": "friend", "friend": hub.friend_json(me) }));
            Some(json!({ "t": "friend", "friend": hub.friend_json(&other) }))
        }
        "decline" => {
            let db = hub.db.lock().unwrap();
            let _ = db.execute("DELETE FROM requests WHERE (src = ?1 AND dst = ?2) OR (src = ?2 AND dst = ?1)", [&other, me]);
            Some(json!({ "t": "declined", "handle": other }))
        }
        "remove" => {
            let db = hub.db.lock().unwrap();
            let _ = db.execute("DELETE FROM friends WHERE (a = ?1 AND b = ?2) OR (a = ?2 AND b = ?1)", [me, &other]);
            let _ = db.execute("DELETE FROM messages WHERE (src = ?1 AND dst = ?2) OR (src = ?2 AND dst = ?1)", [me, &other]);
            drop(db);
            hub.send_to(&other, &json!({ "t": "removed", "handle": me }));
            Some(json!({ "t": "removed", "handle": other }))
        }
        "send" => {
            let to = v["to"].as_str().unwrap_or("").to_lowercase();
            let (id, n, c) = (v["id"].as_str().unwrap_or(""), v["n"].as_str().unwrap_or(""), v["c"].as_str().unwrap_or(""));
            if !hub.are_friends(me, &to) {
                return err("you can only message friends");
            }
            if id.is_empty() || id.len() > 40 || n.len() > 64 || c.is_empty() || c.len() > MAX_MESSAGE || B64.decode(c).is_err() {
                return err("message refused (too long or malformed)");
            }
            let ts = now();
            let db = hub.db.lock().unwrap();
            let queued: i64 = db.query_row("SELECT COUNT(*) FROM messages WHERE dst = ?1", [&to], |r| r.get(0)).unwrap_or(0);
            if queued >= MAX_QUEUED {
                return err(&format!("@{to} has too many messages waiting"));
            }
            let _ = db.execute("INSERT INTO messages (src, dst, cid, n, c, ts) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![me, to, id, n, c, ts]);
            let sid = db.last_insert_rowid();
            drop(db);
            hub.send_to(&to, &json!({ "t": "msg", "sid": sid, "from": me, "id": id, "n": n, "c": c, "ts": ts }));
            Some(json!({ "t": "sent", "to": to, "id": id, "ts": ts }))
        }
        "got" => {
            let db = hub.db.lock().unwrap();
            for sid in v["ids"].as_array().into_iter().flatten().filter_map(|x| x.as_i64()).take(500) {
                let _ = db.execute("DELETE FROM messages WHERE id = ?1 AND dst = ?2", params![sid, me]);
            }
            None
        }
        _ => err("unknown request"),
    }
}

async fn ws(upgrade: WebSocketUpgrade, headers: HeaderMap, ConnectInfo(addr): ConnectInfo<SocketAddr>, State(hub): State<Arc<Hub>>) -> impl IntoResponse {
    // Behind the Cloudflare tunnel every connection comes from this machine; Cloudflare passes the
    // visitor's own address along (only trusted for local connections, i.e. from the tunnel).
    let ip = match headers.get("cf-connecting-ip").and_then(|h| h.to_str().ok()) {
        Some(cf) if addr.ip().is_loopback() => cf.to_string(),
        _ => addr.ip().to_string(),
    };
    upgrade.max_message_size(MAX_FRAME).on_upgrade(move |socket| client(socket, ip, hub))
}

async fn client(socket: WebSocket, ip: String, hub: Arc<Hub>) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    // Writer: everything for this client goes through one channel.
    let writer = tokio::spawn(async move {
        while let Some(text) = rx.recv().await {
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });
    let mut nonce = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut nonce);
    let _ = tx.send(json!({ "t": "challenge", "nonce": B64.encode(nonce) }).to_string());

    let mut me: Option<String> = None;
    let conn_id = {
        let mut n = hub.next.lock().unwrap();
        *n += 1;
        *n
    };
    // Token bucket: RATE frames a second, bursts up to twice that.
    let (mut tokens, mut last) = (RATE * 2.0, Instant::now());
    loop {
        let frame = tokio::select! {
            f = stream.next() => f,
            // Sign in within 30 seconds or go.
            _ = tokio::time::sleep(Duration::from_secs(30)), if me.is_none() => break,
        };
        let Some(Ok(frame)) = frame else { break };
        let Message::Text(text) = frame else { continue };
        tokens = (tokens + last.elapsed().as_secs_f64() * RATE).min(RATE * 2.0);
        last = Instant::now();
        if tokens < 1.0 {
            let _ = tx.send(json!({ "t": "error", "m": "slow down" }).to_string());
            continue;
        }
        tokens -= 1.0;
        let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
        match &me {
            None => {
                if v["t"] != "auth" {
                    continue;
                }
                match sign_in(&hub, &v, &nonce, &ip) {
                    Ok((handle, name)) => {
                        let first = !hub.is_online(&handle);
                        hub.online.lock().unwrap().entry(handle.clone()).or_default().push((conn_id, tx.clone()));
                        let friends: Vec<Value> = hub.friends_of(&handle).iter().map(|f| hub.friend_json(f)).collect();
                        let (incoming, outgoing) = hub.requests(&handle);
                        let _ = tx.send(json!({ "t": "welcome", "handle": handle, "name": name, "friends": friends, "requests": incoming, "sent": outgoing }).to_string());
                        for m in hub.queued(&handle) {
                            let _ = tx.send(m.to_string());
                        }
                        if first {
                            hub.announce(&handle, true);
                        }
                        println!("{ip} signed in as @{handle}");
                        me = Some(handle);
                    }
                    Err(m) => {
                        let _ = tx.send(json!({ "t": "error", "m": m, "auth": true }).to_string());
                    }
                }
            }
            Some(handle) => {
                if let Some(reply) = handle_msg(&hub, handle, &v) {
                    let _ = tx.send(reply.to_string());
                }
            }
        }
    }
    if let Some(handle) = me {
        let last_one = {
            let mut online = hub.online.lock().unwrap();
            let conns = online.entry(handle.clone()).or_default();
            conns.retain(|(id, _)| *id != conn_id);
            let empty = conns.is_empty();
            if empty {
                online.remove(&handle);
            }
            empty
        };
        if last_one {
            hub.announce(&handle, false);
            let _ = hub.db.lock().unwrap().execute("UPDATE users SET seen = ?1 WHERE handle = ?2", params![now(), handle]);
        }
    }
    writer.abort();
}

#[tokio::main]
async fn main() {
    let dir = std::env::var_os("ULTRA_SERVER_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let port: u16 = std::env::var("ULTRA_SERVER_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(7777);
    let hub = Arc::new(Hub { db: Mutex::new(open_db(&dir.join("ultra.db"))), online: Mutex::new(HashMap::new()), next: Mutex::new(0), signups: Mutex::new(HashMap::new()) });
    let app = Router::new()
        .route("/ws", get(ws))
        .route("/", get(|| async { "ULTRA friends & chat relay\n" }))
        .with_state(hub);
    // Only this machine by default: clients come in through the Cloudflare tunnel (TLS), so the
    // relay never speaks unencrypted on the network. ULTRA_SERVER_BIND=:: opens it up (testing).
    let bind = std::env::var("ULTRA_SERVER_BIND").unwrap_or_else(|_| "127.0.0.1".into());
    let listener = tokio::net::TcpListener::bind((bind.as_str(), port)).await.expect("bind");
    println!("ULTRA relay listening on port {port}");
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.expect("serve");
}
