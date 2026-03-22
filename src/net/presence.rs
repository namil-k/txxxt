//! Background presence connection — maintains a persistent TCP connection to
//! the relay server and delivers incoming call notifications.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::mpsc;
use std::time::Duration;

const RELAY_ADDR: &str = "caboose.proxy.rlwy.net:28007";

/// Notification of an incoming call.
pub struct IncomingCall {
    pub caller: String,
    pub code: String,
}

/// Start a background thread that maintains a PRESENCE connection to the relay.
/// Returns a receiver that yields `IncomingCall` events.
pub fn start_presence(token: &str) -> mpsc::Receiver<IncomingCall> {
    let (tx, rx) = mpsc::channel::<IncomingCall>();
    let token = token.to_string();

    std::thread::spawn(move || {
        presence_loop(&token, tx);
    });

    rx
}

fn presence_loop(token: &str, tx: mpsc::Sender<IncomingCall>) {
    loop {
        match connect_presence(token, &tx) {
            Ok(()) => {
                // Clean disconnect — retry after a short pause.
                std::thread::sleep(Duration::from_secs(5));
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                // Auth failed — stop retrying (bad token).
                break;
            }
            Err(_) => {
                // Network error — retry after a pause.
                std::thread::sleep(Duration::from_secs(5));
            }
        }
        // If the receiver is dropped (TUI exited), stop retrying.
        // Use a zero-length probe — just check if the channel is still alive.
        if tx.send(IncomingCall { caller: String::new(), code: String::new() }).is_err() {
            break;
        }
        // Note: empty caller/code probe is filtered out in TUI (see below).
    }
}

fn connect_presence(token: &str, tx: &mpsc::Sender<IncomingCall>) -> std::io::Result<()> {
    let mut stream = TcpStream::connect(RELAY_ADDR)?;
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;

    // Send AUTH
    write!(stream, "AUTH {}\n", token)?;
    stream.flush()?;

    // Read OK <username> or ERR
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut response = String::new();
    reader.read_line(&mut response)?;
    let response = response.trim();

    if !response.starts_with("OK ") {
        // Auth failed — bad/expired token. Stop silently.
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "auth failed"));
    }

    // Send PRESENCE
    write!(stream, "PRESENCE\n")?;
    stream.flush()?;

    // Loop: read incoming messages.
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // disconnected
            Ok(_) => {
                let msg = line.trim();
                if msg == "PING" {
                    continue; // ignore keepalives
                }
                // INCOMING <caller> <code>
                if let Some(rest) = msg.strip_prefix("INCOMING ") {
                    let parts: Vec<&str> = rest.splitn(2, ' ').collect();
                    if parts.len() == 2 {
                        let call = IncomingCall {
                            caller: parts[0].to_string(),
                            code: parts[1].to_string(),
                        };
                        if tx.send(call).is_err() {
                            // Receiver dropped — TUI exited.
                            return Ok(());
                        }
                    }
                }
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue; // read timeout — connection still alive, loop
            }
            Err(e) => return Err(e),
        }
    }

    Ok(())
}
