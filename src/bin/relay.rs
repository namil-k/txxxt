//! txxxt relay server — pairs two clients and relays data between them.
//!
//! Protocol (each client sends one command line, newline-terminated):
//!
//! Unauthenticated:
//!   CREATE → ROOM <code>      (free tier, 5-min session)
//!   JOIN <code> → OK
//!   REGISTER <key> <username> → OK / ERR <msg>
//!   LOGIN <license_key> → SESSION <token> <username> / ERR <msg>
//!
//! Authenticated (client sends AUTH <token> first, server responds OK <username> / ERR):
//!   PRESENCE → (keep-alive, receives push messages)
//!   CALL <username> → ROOM <code> / ERR <msg>
//!   FRIENDS ADD <username> → OK / ERR <msg>
//!   FRIENDS REMOVE <username> → OK / ERR <msg>
//!   FRIENDS LIST → FRIENDS <u1>,<u2>,... / FRIENDS

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sqlx::PgPool;
use tokio::sync::mpsc::UnboundedSender;

/// Maximum concurrent connections.
const MAX_CONNECTIONS: usize = 200;
/// Room code length.
const CODE_LEN: usize = 6;
/// Session time limit (free tier).
const SESSION_LIMIT: Duration = Duration::from_secs(5 * 60);
/// Plus tier session limit (essentially unlimited).
const PLUS_SESSION_LIMIT: Duration = Duration::from_secs(86400 * 365);

/// Pending room: a creator waiting for a peer.
struct PendingRoom {
    creator_stream: TcpStream,
    created_at: Instant,
    session_limit: Duration,
}

type Rooms = Arc<Mutex<HashMap<String, PendingRoom>>>;
type Presence = Arc<Mutex<HashMap<String, UnboundedSender<String>>>>;

#[tokio::main]
async fn main() {
    let port = std::env::var("PORT").unwrap_or_else(|_| "9090".into());
    let addr = format!("0.0.0.0:{}", port);

    // Connect to PostgreSQL.
    let db_url = std::env::var("DATABASE_PRIVATE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .expect("DATABASE_PRIVATE_URL or DATABASE_URL must be set");
    let pool = PgPool::connect(&db_url).await.expect("failed to connect to database");

    // Run migrations / create tables.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS users (
            license_key TEXT PRIMARY KEY,
            username    TEXT UNIQUE NOT NULL,
            created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )"
    ).execute(&pool).await.expect("failed to create users table");

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS friends (
            user_key   TEXT NOT NULL REFERENCES users(license_key),
            friend_key TEXT NOT NULL REFERENCES users(license_key),
            PRIMARY KEY (user_key, friend_key)
        )"
    ).execute(&pool).await.expect("failed to create friends table");

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS sessions (
            token       TEXT PRIMARY KEY,
            license_key TEXT NOT NULL REFERENCES users(license_key),
            created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )"
    ).execute(&pool).await.expect("failed to create sessions table");

    println!("relay server listening on {}", addr);

    let rooms: Rooms = Arc::new(Mutex::new(HashMap::new()));
    let presence: Presence = Arc::new(Mutex::new(HashMap::new()));
    let active_conns = Arc::new(AtomicUsize::new(0));

    // Periodic cleanup of stale rooms.
    let rooms_cleanup = rooms.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let mut map = rooms_cleanup.lock().unwrap();
            map.retain(|code, room| {
                let stale = room.created_at.elapsed() > Duration::from_secs(120);
                if stale {
                    println!("expired room {}", code);
                }
                !stale
            });
        }
    });

    let listener = tokio::net::TcpListener::bind(&addr).await.expect("failed to bind");

    loop {
        match listener.accept().await {
            Ok((tokio_stream, _peer_addr)) => {
                let current = active_conns.load(Ordering::Relaxed);
                if current >= MAX_CONNECTIONS {
                    eprintln!("connection limit reached, rejecting");
                    drop(tokio_stream);
                    continue;
                }
                active_conns.fetch_add(1, Ordering::Relaxed);

                // Convert tokio stream to std for use in sync threads.
                let std_stream = tokio_stream.into_std().expect("failed to convert stream");
                std_stream.set_nonblocking(false).expect("failed to set blocking mode");

                let rooms = rooms.clone();
                let presence = presence.clone();
                let conns = active_conns.clone();
                let pool = pool.clone();

                tokio::task::spawn_blocking(move || {
                    if let Err(e) = handle_client(std_stream, rooms, presence, pool) {
                        eprintln!("client error: {}", e);
                    }
                    conns.fetch_sub(1, Ordering::Relaxed);
                });
            }
            Err(e) => eprintln!("accept error: {}", e),
        }
    }
}

fn handle_client(
    mut stream: TcpStream,
    rooms: Rooms,
    presence: Presence,
    pool: PgPool,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.len() > 512 {
        write!(stream, "ERR command too long\n")?;
        return Ok(());
    }
    let cmd = line.trim().to_string();

    // ── Unauthenticated commands ─────────────────────────────────────────────

    if cmd == "CREATE" {
        return handle_create(stream, rooms, SESSION_LIMIT);
    }

    if let Some(code) = cmd.strip_prefix("JOIN ") {
        return handle_join(stream, rooms, code.trim().to_uppercase());
    }

    if let Some(rest) = cmd.strip_prefix("REGISTER ") {
        return handle_register(stream, rest.trim(), pool);
    }

    if let Some(key) = cmd.strip_prefix("LOGIN ") {
        return handle_login(stream, key.trim(), pool);
    }

    // ── Authenticated commands ───────────────────────────────────────────────

    if let Some(token) = cmd.strip_prefix("AUTH ") {
        let token = token.trim().to_string();
        // Look up session.
        let row = tokio::runtime::Handle::current().block_on(async {
            sqlx::query_as::<_, (String, String)>(
                "SELECT s.license_key, u.username
                 FROM sessions s
                 JOIN users u ON u.license_key = s.license_key
                 WHERE s.token = $1"
            )
            .bind(&token)
            .fetch_optional(&pool)
            .await
        });

        let (license_key, username) = match row {
            Ok(Some((lk, un))) => (lk, un),
            _ => {
                write!(stream, "ERR invalid session\n")?;
                stream.flush()?;
                return Ok(());
            }
        };

        write!(stream, "OK {}\n", username)?;
        stream.flush()?;

        // Read next command.
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        let mut reader2 = BufReader::new(stream.try_clone()?);
        let mut line2 = String::new();
        reader2.read_line(&mut line2)?;
        let subcmd = line2.trim().to_string();

        if subcmd == "PRESENCE" {
            return handle_presence(stream, presence, &username);
        }

        if let Some(target_username) = subcmd.strip_prefix("CALL ") {
            return handle_call(stream, rooms, presence, target_username.trim(), &username, PLUS_SESSION_LIMIT);
        }

        if subcmd == "FRIENDS LIST" {
            return handle_friends_list(stream, &license_key, pool);
        }

        if let Some(rest) = subcmd.strip_prefix("FRIENDS ADD ") {
            return handle_friends_add(stream, &license_key, rest.trim(), pool);
        }

        if let Some(rest) = subcmd.strip_prefix("FRIENDS REMOVE ") {
            return handle_friends_remove(stream, &license_key, rest.trim(), pool);
        }

        write!(stream, "ERR unknown authenticated command\n")?;
        stream.flush()?;
        return Ok(());
    }

    write!(stream, "ERR unknown command\n")?;
    stream.flush()?;
    Ok(())
}

fn handle_create(mut stream: TcpStream, rooms: Rooms, session_limit: Duration) -> std::io::Result<()> {
    let code = {
        let mut map = rooms.lock().unwrap();
        let code = loop {
            let c = gen_code();
            if !map.contains_key(&c) {
                break c;
            }
        };
        write!(stream, "ROOM {}\n", code)?;
        stream.flush()?;
        println!("room {} created", code);
        map.insert(code.clone(), PendingRoom {
            creator_stream: stream.try_clone()?,
            created_at: Instant::now(),
            session_limit,
        });
        code
    };

    // Wait for peer to join.
    stream.set_nonblocking(true).ok();
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let mut peek_buf = [0u8; 1];
        match std::io::Read::read(&mut stream, &mut peek_buf) {
            Ok(0) => {
                rooms.lock().unwrap().remove(&code);
                println!("room {} creator disconnected", code);
                return Ok(());
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => {
                rooms.lock().unwrap().remove(&code);
                return Ok(());
            }
            Ok(_) => {}
        }
        if !rooms.lock().unwrap().contains_key(&code) {
            println!("room {} matched, creator thread done", code);
            return Ok(());
        }
    }
}

fn handle_join(mut stream: TcpStream, rooms: Rooms, code: String) -> std::io::Result<()> {
    let room = {
        let mut map = rooms.lock().unwrap();
        match map.remove(&code) {
            Some(r) => r,
            None => {
                write!(stream, "ERR room not found\n")?;
                stream.flush()?;
                return Ok(());
            }
        }
    };

    write!(stream, "OK\n")?;
    stream.flush()?;
    let mut creator = room.creator_stream;
    write!(creator, "PAIRED\n")?;
    creator.flush()?;

    println!("room {} paired — relaying", code);
    relay(creator, stream, room.session_limit);
    println!("room {} session ended", code);
    Ok(())
}

fn handle_register(mut stream: TcpStream, rest: &str, pool: PgPool) -> std::io::Result<()> {
    let parts: Vec<&str> = rest.splitn(2, ' ').collect();
    if parts.len() != 2 {
        write!(stream, "ERR usage: REGISTER <license_key> <username>\n")?;
        stream.flush()?;
        return Ok(());
    }
    let license_key = parts[0];
    let username = parts[1].trim();

    // Validate username (alphanumeric + underscore, 3-20 chars).
    if username.len() < 3 || username.len() > 20 {
        write!(stream, "ERR username must be 3-20 characters\n")?;
        stream.flush()?;
        return Ok(());
    }
    if !username.chars().all(|c| c.is_alphanumeric() || c == '_') {
        write!(stream, "ERR username may only contain letters, numbers, and underscores\n")?;
        stream.flush()?;
        return Ok(());
    }

    // Validate license key via Lemon Squeezy.
    if !validate_license_key(license_key) {
        write!(stream, "ERR invalid or already used license key\n")?;
        stream.flush()?;
        return Ok(());
    }

    // Insert into DB.
    let rt = tokio::runtime::Handle::current();
    let result = rt.block_on(async {
        sqlx::query(
            "INSERT INTO users (license_key, username) VALUES ($1, $2)"
        )
        .bind(license_key)
        .bind(username)
        .execute(&pool)
        .await
    });

    match result {
        Ok(_) => {
            println!("registered user {} with key {}", username, &license_key[..8]);
            write!(stream, "OK\n")?;
        }
        Err(sqlx::Error::Database(e)) if e.message().contains("unique") || e.message().contains("duplicate") => {
            write!(stream, "ERR username or license key already registered\n")?;
        }
        Err(e) => {
            eprintln!("register error: {}", e);
            write!(stream, "ERR server error\n")?;
        }
    }
    stream.flush()?;
    Ok(())
}

fn handle_login(mut stream: TcpStream, license_key: &str, pool: PgPool) -> std::io::Result<()> {
    let rt = tokio::runtime::Handle::current();

    // Look up user.
    let row = rt.block_on(async {
        sqlx::query_as::<_, (String,)>(
            "SELECT username FROM users WHERE license_key = $1"
        )
        .bind(license_key)
        .fetch_optional(&pool)
        .await
    });

    let username = match row {
        Ok(Some((un,))) => un,
        Ok(None) => {
            write!(stream, "ERR license key not registered\n")?;
            stream.flush()?;
            return Ok(());
        }
        Err(e) => {
            eprintln!("login error: {}", e);
            write!(stream, "ERR server error\n")?;
            stream.flush()?;
            return Ok(());
        }
    };

    // Generate session token.
    let token = uuid::Uuid::new_v4().to_string();

    let insert = rt.block_on(async {
        sqlx::query(
            "INSERT INTO sessions (token, license_key) VALUES ($1, $2)"
        )
        .bind(&token)
        .bind(license_key)
        .execute(&pool)
        .await
    });

    match insert {
        Ok(_) => {
            write!(stream, "SESSION {} {}\n", token, username)?;
        }
        Err(e) => {
            eprintln!("session insert error: {}", e);
            write!(stream, "ERR server error\n")?;
        }
    }
    stream.flush()?;
    Ok(())
}

fn handle_presence(mut stream: TcpStream, presence: Presence, username: &str) -> std::io::Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    presence.lock().unwrap().insert(username.to_string(), tx);

    let username_owned = username.to_string();
    println!("user {} is now ONLINE", username_owned);

    // Spawn a thread to forward messages from the channel to the TCP stream.
    let mut stream_write = stream.try_clone()?;
    let presence_cleanup = presence.clone();
    let username_cleanup = username_owned.clone();

    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();

    std::thread::spawn(move || {
        // We use blocking recv in a loop. Since it's a tokio unbounded channel,
        // we need to use a blocking_recv approach inside a tokio context.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            loop {
                tokio::select! {
                    msg = rx.recv() => {
                        match msg {
                            Some(m) => {
                                if write!(stream_write, "{}", m).is_err() {
                                    break;
                                }
                                let _ = stream_write.flush();
                            }
                            None => break,
                        }
                    }
                }
            }
        });
        presence_cleanup.lock().unwrap().remove(&username_cleanup);
        println!("user {} went OFFLINE", username_cleanup);
        let _ = stop_tx.send(());
    });

    // Send periodic PINGs so the connection stays alive.
    stream.set_read_timeout(Some(Duration::from_secs(35))).ok();
    let ping_interval = Duration::from_secs(30);
    let mut last_ping = Instant::now();

    loop {
        // Check if writer thread stopped.
        if stop_rx.try_recv().is_ok() {
            break;
        }

        if last_ping.elapsed() >= ping_interval {
            if write!(stream, "PING\n").is_err() {
                break;
            }
            stream.flush().ok();
            last_ping = Instant::now();
        }

        // Sleep briefly.
        std::thread::sleep(Duration::from_secs(1));
    }

    presence.lock().unwrap().remove(username);
    Ok(())
}

fn handle_call(
    mut stream: TcpStream,
    rooms: Rooms,
    presence: Presence,
    target_username: &str,
    caller_username: &str,
    session_limit: Duration,
) -> std::io::Result<()> {
    // Check if target is online.
    let target_tx = presence.lock().unwrap().get(target_username).cloned();
    let Some(target_tx) = target_tx else {
        write!(stream, "ERR user not online\n")?;
        stream.flush()?;
        return Ok(());
    };

    // Generate room code.
    let code = {
        let mut map = rooms.lock().unwrap();
        let c = loop {
            let c = gen_code();
            if !map.contains_key(&c) {
                break c;
            }
        };
        write!(stream, "ROOM {}\n", &c)?;
        stream.flush()?;
        println!("room {} created for CALL from {} to {}", c, caller_username, target_username);
        map.insert(c.clone(), PendingRoom {
            creator_stream: stream.try_clone()?,
            created_at: Instant::now(),
            session_limit,
        });
        c
    };

    // Push INCOMING message to target's presence channel.
    let incoming_msg = format!("INCOMING {} {}\n", caller_username, code);
    let _ = target_tx.send(incoming_msg);

    // Now wait for the peer to join (same loop as CREATE).
    stream.set_nonblocking(true).ok();
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let mut peek_buf = [0u8; 1];
        match std::io::Read::read(&mut stream, &mut peek_buf) {
            Ok(0) => {
                rooms.lock().unwrap().remove(&code);
                println!("room {} caller disconnected", code);
                return Ok(());
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => {
                rooms.lock().unwrap().remove(&code);
                return Ok(());
            }
            Ok(_) => {}
        }
        if !rooms.lock().unwrap().contains_key(&code) {
            println!("room {} CALL matched, caller thread done", code);
            return Ok(());
        }
    }
}

fn handle_friends_list(mut stream: TcpStream, license_key: &str, pool: PgPool) -> std::io::Result<()> {
    let rt = tokio::runtime::Handle::current();
    let rows = rt.block_on(async {
        sqlx::query_as::<_, (String,)>(
            "SELECT u.username FROM friends f
             JOIN users u ON u.license_key = f.friend_key
             WHERE f.user_key = $1
             ORDER BY u.username"
        )
        .bind(license_key)
        .fetch_all(&pool)
        .await
    });

    match rows {
        Ok(r) if r.is_empty() => {
            write!(stream, "FRIENDS\n")?;
        }
        Ok(r) => {
            let names: Vec<String> = r.into_iter().map(|(un,)| un).collect();
            write!(stream, "FRIENDS {}\n", names.join(","))?;
        }
        Err(e) => {
            eprintln!("friends list error: {}", e);
            write!(stream, "ERR server error\n")?;
        }
    }
    stream.flush()?;
    Ok(())
}

fn handle_friends_add(mut stream: TcpStream, license_key: &str, friend_username: &str, pool: PgPool) -> std::io::Result<()> {
    let rt = tokio::runtime::Handle::current();

    // Look up friend's license key.
    let friend_key = rt.block_on(async {
        sqlx::query_as::<_, (String,)>(
            "SELECT license_key FROM users WHERE username = $1"
        )
        .bind(friend_username)
        .fetch_optional(&pool)
        .await
    });

    let friend_key = match friend_key {
        Ok(Some((k,))) => k,
        Ok(None) => {
            write!(stream, "ERR user not found\n")?;
            stream.flush()?;
            return Ok(());
        }
        Err(e) => {
            eprintln!("friends add lookup error: {}", e);
            write!(stream, "ERR server error\n")?;
            stream.flush()?;
            return Ok(());
        }
    };

    if friend_key == license_key {
        write!(stream, "ERR cannot friend yourself\n")?;
        stream.flush()?;
        return Ok(());
    }

    let insert = rt.block_on(async {
        sqlx::query(
            "INSERT INTO friends (user_key, friend_key) VALUES ($1, $2) ON CONFLICT DO NOTHING"
        )
        .bind(license_key)
        .bind(&friend_key)
        .execute(&pool)
        .await
    });

    match insert {
        Ok(_) => write!(stream, "OK\n")?,
        Err(e) => {
            eprintln!("friends add error: {}", e);
            write!(stream, "ERR server error\n")?;
        }
    }
    stream.flush()?;
    Ok(())
}

fn handle_friends_remove(mut stream: TcpStream, license_key: &str, friend_username: &str, pool: PgPool) -> std::io::Result<()> {
    let rt = tokio::runtime::Handle::current();

    let result = rt.block_on(async {
        sqlx::query(
            "DELETE FROM friends
             WHERE user_key = $1
             AND friend_key = (SELECT license_key FROM users WHERE username = $2)"
        )
        .bind(license_key)
        .bind(friend_username)
        .execute(&pool)
        .await
    });

    match result {
        Ok(_) => write!(stream, "OK\n")?,
        Err(e) => {
            eprintln!("friends remove error: {}", e);
            write!(stream, "ERR server error\n")?;
        }
    }
    stream.flush()?;
    Ok(())
}

/// Validate a Lemon Squeezy license key via their API.
fn validate_license_key(key: &str) -> bool {
    use std::process::Command;

    let output = Command::new("curl")
        .args([
            "-sSL", "--max-time", "10",
            "-X", "POST",
            "-H", "Content-Type: application/x-www-form-urlencoded",
            "-d", &format!("license_key={}", key),
            "https://api.lemonsqueezy.com/v1/licenses/validate",
        ])
        .output();

    match output {
        Ok(o) => {
            let body = String::from_utf8_lossy(&o.stdout);
            body.contains("\"valid\":true") || body.contains("\"valid\": true")
        }
        Err(_) => false,
    }
}

/// Relay data bidirectionally between two streams until timeout or disconnect.
fn relay(stream_a: TcpStream, stream_b: TcpStream, timeout: Duration) {
    let start = Instant::now();

    stream_a.set_read_timeout(Some(Duration::from_millis(50))).ok();
    stream_b.set_read_timeout(Some(Duration::from_millis(50))).ok();
    stream_a.set_nonblocking(false).ok();
    stream_b.set_nonblocking(false).ok();

    let a_to_b_a = stream_a.try_clone().unwrap();
    let a_to_b_b = stream_b.try_clone().unwrap();
    let b_to_a_a = stream_a.try_clone().unwrap();
    let b_to_a_b = stream_b.try_clone().unwrap();

    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done1 = done.clone();
    let done2 = done.clone();

    let t1 = std::thread::spawn(move || {
        pipe(a_to_b_a, a_to_b_b, &done1, start, timeout);
        done1.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    let t2 = std::thread::spawn(move || {
        pipe(b_to_a_b, b_to_a_a, &done2, start, timeout);
        done2.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    t1.join().ok();
    t2.join().ok();
}

/// Copy data from src to dst until done flag, timeout, or error.
fn pipe(
    mut src: TcpStream,
    mut dst: TcpStream,
    done: &std::sync::atomic::AtomicBool,
    start: Instant,
    timeout: Duration,
) {
    let mut buf = [0u8; 8192];
    loop {
        if done.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        if start.elapsed() > timeout {
            break;
        }
        match std::io::Read::read(&mut src, &mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if dst.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(_) => break,
        }
    }
}

/// Generate a random room code.
fn gen_code() -> String {
    let chars = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut code = String::with_capacity(CODE_LEN);
    let mut bytes = [0u8; CODE_LEN];

    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut f, &mut bytes);
    } else {
        use std::time::SystemTime;
        let seed = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let mut s = seed;
        for b in bytes.iter_mut() {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *b = (s >> 33) as u8;
        }
    }

    for &b in &bytes {
        code.push(chars[b as usize % chars.len()] as char);
    }
    code
}
