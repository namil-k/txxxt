use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

use anyhow::Result;

pub const RELAY_ADDR: &str = "caboose.proxy.rlwy.net:28007";

fn connect() -> Result<TcpStream> {
    use std::net::ToSocketAddrs;
    let addr = RELAY_ADDR
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow::anyhow!("cannot resolve relay address"))?;
    Ok(TcpStream::connect(addr)?)
}

fn send_and_read(cmd: &str) -> Result<String> {
    let mut stream = connect()?;
    write!(stream, "{}\n", cmd)?;
    stream.flush()?;
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;
    Ok(response.trim().to_string())
}

/// Authenticate and send a subcommand. Returns the response line.
fn auth_cmd(token: &str, subcmd: &str) -> Result<String> {
    let mut stream = connect()?;

    // AUTH
    write!(stream, "AUTH {}\n", token)?;
    stream.flush()?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut auth_resp = String::new();
    reader.read_line(&mut auth_resp)?;
    let auth_resp = auth_resp.trim().to_string();
    if !auth_resp.starts_with("OK ") {
        anyhow::bail!("auth failed: {}", auth_resp);
    }

    // Subcommand
    write!(stream, "{}\n", subcmd)?;
    stream.flush()?;
    let mut resp = String::new();
    reader.read_line(&mut resp)?;
    Ok(resp.trim().to_string())
}

/// Register a username with a license key. Also logs in on success.
/// Returns (username, token) on success.
pub fn register(key: &str, username: &str) -> Result<(String, String)> {
    let resp = send_and_read(&format!("REGISTER {} {}", key, username))?;
    if resp == "OK" {
        // Auto-login after register.
        login(key)
    } else {
        anyhow::bail!("{}", resp.strip_prefix("ERR ").unwrap_or(&resp));
    }
}

/// Login with a license key. Returns (username, token).
pub fn login(key: &str) -> Result<(String, String)> {
    let resp = send_and_read(&format!("LOGIN {}", key))?;
    if let Some(rest) = resp.strip_prefix("SESSION ") {
        let parts: Vec<&str> = rest.splitn(2, ' ').collect();
        if parts.len() == 2 {
            Ok((parts[1].to_string(), parts[0].to_string()))
        } else {
            anyhow::bail!("unexpected response: {}", resp);
        }
    } else {
        anyhow::bail!("{}", resp.strip_prefix("ERR ").unwrap_or(&resp));
    }
}

/// Get friends list. Returns list of usernames.
pub fn friends_list(token: &str) -> Result<Vec<String>> {
    let resp = auth_cmd(token, "FRIENDS LIST")?;
    if resp == "FRIENDS" {
        Ok(vec![])
    } else if let Some(names) = resp.strip_prefix("FRIENDS ") {
        Ok(names.split(',').map(|s| s.trim().to_string()).collect())
    } else {
        anyhow::bail!("{}", resp.strip_prefix("ERR ").unwrap_or(&resp));
    }
}

/// Add a friend.
pub fn friends_add(token: &str, username: &str) -> Result<()> {
    let resp = auth_cmd(token, &format!("FRIENDS ADD {}", username))?;
    if resp == "OK" {
        Ok(())
    } else {
        anyhow::bail!("{}", resp.strip_prefix("ERR ").unwrap_or(&resp));
    }
}

/// Remove a friend.
pub fn friends_remove(token: &str, username: &str) -> Result<()> {
    let resp = auth_cmd(token, &format!("FRIENDS REMOVE {}", username))?;
    if resp == "OK" {
        Ok(())
    } else {
        anyhow::bail!("{}", resp.strip_prefix("ERR ").unwrap_or(&resp));
    }
}

/// Call a user by username. Returns the room code.
pub fn call_user(token: &str, username: &str) -> Result<String> {
    let resp = auth_cmd(token, &format!("CALL {}", username))?;
    if let Some(code) = resp.strip_prefix("ROOM ") {
        Ok(code.trim().to_string())
    } else {
        anyhow::bail!("{}", resp.strip_prefix("ERR ").unwrap_or(&resp));
    }
}
