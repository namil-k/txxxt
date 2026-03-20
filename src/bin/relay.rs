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
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Room code length.
const CODE_LEN: usize = 4;

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
                let rooms = rooms.clone();
                std::thread::spawn(move || {
                    if let Err(e) = handle_client(stream, rooms) {
                        eprintln!("client error: {}", e);
                    }
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
        // So we just wait here — if we're still in the map, keep waiting.
        // Once removed, this thread's stream clone becomes the creator side.
        loop {
            std::thread::sleep(Duration::from_millis(200));
            let map = rooms.lock().unwrap();
            if !map.contains_key(&code) {
                // Peer joined — the JOIN thread is handling relay.
                // This thread can exit; the stream clone lives in the relay.
                println!("room {} matched, creator thread done", code);
                return Ok(());
            }
            // Check if creator disconnected while waiting.
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

/// Atomic counter for unique seeds.
static CODE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Generate a random 4-character uppercase code.
fn gen_code() -> String {
    use std::time::SystemTime;
    let counter = CODE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
        ^ (counter.wrapping_mul(2654435761)); // Knuth's multiplicative hash

    let chars = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789"; // no I/O/0/1 to avoid confusion
    let mut code = String::with_capacity(CODE_LEN);
    let mut s = seed;
    for _ in 0..CODE_LEN {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        code.push(chars[(s >> 33) as usize % chars.len()] as char);
    }
    code
}
