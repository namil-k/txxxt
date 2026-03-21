//! txxxt relay server — pairs two clients and relays data between them.
//!
//! Protocol:
//!   Client sends a single command line (UTF-8, newline-terminated):
//!     "CREATE\n"       → server responds "ROOM <code>\n", waits for peer
//!     "JOIN <code>\n"  → server responds "OK\n" on match, "ERR <msg>\n" on failure
//!
//!   After both peers are connected, the server relays raw bytes
//!   bidirectionally until timeout or disconnect.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Maximum concurrent connections.
const MAX_CONNECTIONS: usize = 200;

/// Room code length (6 chars = ~900M combinations).
const CODE_LEN: usize = 6;

/// Session time limit (free tier).
const SESSION_LIMIT: Duration = Duration::from_secs(5 * 60);

/// Pending room: a creator waiting for a peer to join.
struct PendingRoom {
    creator_stream: TcpStream,
    created_at: Instant,
}

type Rooms = Arc<Mutex<HashMap<String, PendingRoom>>>;

fn main() {
    let port = std::env::var("PORT").unwrap_or_else(|_| "9090".into());
    let addr = format!("0.0.0.0:{}", port);
    let listener = TcpListener::bind(&addr).expect("failed to bind");
    println!("relay server listening on {}", addr);

    let rooms: Rooms = Arc::new(Mutex::new(HashMap::new()));
    let active_conns = Arc::new(AtomicUsize::new(0));

    // Periodic cleanup of stale rooms (>2 min waiting).
    let rooms_cleanup = rooms.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(30));
        let mut map = rooms_cleanup.lock().unwrap();
        map.retain(|code, room| {
            let stale = room.created_at.elapsed() > Duration::from_secs(120);
            if stale {
                println!("expired room {}", code);
            }
            !stale
        });
    });

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let current = active_conns.load(Ordering::Relaxed);
                if current >= MAX_CONNECTIONS {
                    eprintln!("connection limit reached ({}/{}), rejecting", current, MAX_CONNECTIONS);
                    let _ = write!(&stream, "ERR server full\n");
                    continue;
                }
                active_conns.fetch_add(1, Ordering::Relaxed);
                let rooms = rooms.clone();
                let conns = active_conns.clone();
                std::thread::spawn(move || {
                    if let Err(e) = handle_client(stream, rooms) {
                        eprintln!("client error: {}", e);
                    }
                    conns.fetch_sub(1, Ordering::Relaxed);
                });
            }
            Err(e) => eprintln!("accept error: {}", e),
        }
    }
}

fn handle_client(mut stream: TcpStream, rooms: Rooms) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;

    // Read the command line.
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.len() > 256 {
        write!(stream, "ERR command too long\n")?;
        return Ok(());
    }
    let cmd = line.trim();

    if cmd == "CREATE" {
        // Generate a unique room code.
        let code = {
            let mut map = rooms.lock().unwrap();
            let code = loop {
                let c = gen_code();
                if !map.contains_key(&c) {
                    break c;
                }
            };
            // Respond with the room code.
            write!(stream, "ROOM {}\n", code)?;
            stream.flush()?;
            println!("room {} created", code);

            map.insert(
                code.clone(),
                PendingRoom {
                    creator_stream: stream.try_clone()?,
                    created_at: Instant::now(),
                },
            );
            code
        };

        // Wait for the peer to join (poll until removed from map).
        // The JOIN handler will remove the room and start relaying.
        // Once removed, this thread's stream clone becomes the creator side.
        //
        // Also detect if the creator disconnected while waiting:
        // peek() returns Ok(0) on clean disconnect, Err on broken pipe.
        stream.set_nonblocking(true).ok();
        loop {
            std::thread::sleep(Duration::from_millis(200));

            // Check if creator disconnected.
            let mut peek_buf = [0u8; 1];
            match std::io::Read::read(&mut stream, &mut peek_buf) {
                Ok(0) => {
                    // Creator closed connection — clean up the room.
                    let mut map = rooms.lock().unwrap();
                    map.remove(&code);
                    println!("room {} creator disconnected, removed", code);
                    return Ok(());
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // No data yet — still connected, keep waiting.
                }
                Err(_) => {
                    // Connection error — clean up.
                    let mut map = rooms.lock().unwrap();
                    map.remove(&code);
                    println!("room {} creator error, removed", code);
                    return Ok(());
                }
                Ok(_) => {
                    // Unexpected data — ignore, keep waiting.
                }
            }

            let map = rooms.lock().unwrap();
            if !map.contains_key(&code) {
                // Peer joined — the JOIN thread is handling relay.
                println!("room {} matched, creator thread done", code);
                return Ok(());
            }
            drop(map);
        }
    } else if let Some(code) = cmd.strip_prefix("JOIN ") {
        let code = code.trim().to_uppercase();

        // Look up the room.
        let creator_stream = {
            let mut map = rooms.lock().unwrap();
            match map.remove(&code) {
                Some(room) => room.creator_stream,
                None => {
                    write!(stream, "ERR room not found\n")?;
                    stream.flush()?;
                    return Ok(());
                }
            }
        };

        write!(stream, "OK\n")?;
        stream.flush()?;
        // Also notify creator that peer joined.
        let mut creator = creator_stream;
        write!(creator, "PAIRED\n")?;
        creator.flush()?;

        println!("room {} paired — relaying", code);

        // Start bidirectional relay.
        relay(creator, stream, SESSION_LIMIT);

        println!("room {} session ended", code);
    } else {
        write!(stream, "ERR unknown command\n")?;
        stream.flush()?;
    }

    Ok(())
}

/// Relay data bidirectionally between two streams until timeout or disconnect.
fn relay(stream_a: TcpStream, stream_b: TcpStream, timeout: Duration) {
    let start = Instant::now();

    // Set short read timeouts so we can check the session timer.
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
    let timeout1 = timeout;
    let timeout2 = timeout;
    let start1 = start;
    let start2 = start;

    // A → B
    let t1 = std::thread::spawn(move || {
        pipe(a_to_b_a, a_to_b_b, &done1, start1, timeout1);
        done1.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    // B → A
    let t2 = std::thread::spawn(move || {
        pipe(b_to_a_b, b_to_a_a, &done2, start2, timeout2);
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
            // Send timeout notice (best-effort).
            let _ = dst.write_all(b"");
            break;
        }
        match std::io::Read::read(&mut src, &mut buf) {
            Ok(0) => break, // disconnected
            Ok(n) => {
                if dst.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue; // timeout on read — just loop
            }
            Err(_) => break,
        }
    }
}

/// Generate a random room code using OS entropy.
fn gen_code() -> String {
    let chars = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789"; // no I/O/0/1 to avoid confusion
    let mut code = String::with_capacity(CODE_LEN);
    let mut bytes = [0u8; CODE_LEN];

    // Read from OS random source.
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut f, &mut bytes);
    } else {
        // Fallback: time-based (less secure but functional on all platforms).
        use std::time::SystemTime;
        let seed = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let mut s = seed;
        for b in bytes.iter_mut() {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            *b = (s >> 33) as u8;
        }
    }

    for &b in &bytes {
        code.push(chars[b as usize % chars.len()] as char);
    }
    code
}
