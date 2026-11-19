//! The emulator's HTTP client.
//!
//! It is small on purpose. Zeebo games speak **plain HTTP** — Zeeboids points at
//! `http://www.zeeboids.com/...` and there is no HTTPS anywhere in the module —, so a whole
//! library would bring TLS, redirects, `keep-alive` and policies that nobody here uses. What is
//! needed is a POST with a binary body and the response back.
//!
//! Giving network access to a binary from an outside source is a design decision, and so it is not
//! silent: every request goes into the report, and the [`crate::machine::Machine`] can be created
//! without it.

#[cfg(not(target_os = "emscripten"))]
use std::io::{Read, Write};
#[cfg(not(target_os = "emscripten"))]
use std::net::TcpStream;
#[cfg(not(target_os = "emscripten"))]
use std::time::Duration;

/// How long to wait for the connection and for the response.
///
/// Five seconds is short for a real server, and that is exactly the point: the emulator cannot
/// stall because the other side does not answer. The game already has its own timeout, and
/// failing fast hands control back to it.
#[cfg(not(target_os = "emscripten"))]
const TIMEOUT: Duration = Duration::from_secs(5);

/// Largest response we accept to keep.
#[cfg(not(target_os = "emscripten"))]
const MAX_RESPONSE: usize = 1 << 20;

/// What came back from the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Splits `http://machine[:port]/path` into (machine, port, path).
///
/// Only `http`. An `https://` here would be a request we cannot serve, and answering as if we
/// could would be worse than refusing.
pub fn split_url(url: &str) -> Result<(String, u16, String), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("not an http URL: {url:?}"))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (machine, port) = match authority.rsplit_once(':') {
        Some((m, p)) => (
            m,
            p.parse::<u16>()
                .map_err(|_| format!("invalid port in {url:?}"))?,
        ),
        None => (authority, 80),
    };
    if machine.is_empty() {
        return Err(format!("no host in {url:?}"));
    }
    Ok((machine.to_string(), port, path.to_string()))
}

/// Sends a POST and returns what came back.
///
/// The `Content-Type` is `application/octet-stream` because that is what the game declares — the
/// three strings that describe the request in the Zeeboids module are `POST`, `X-Method: POST` and
/// that type.
///
/// The `delta` changes **where** the connection goes, without touching what the game asked for: the
/// `Host` and the path stay those of the original URL. This is how a game is pointed at a private
/// server — by changing the emulator's configuration, not by patching the game binary in memory. It
/// also allows testing without privileges, since port 80 requires root.
pub fn post(url: &str, body: &[u8], delta: Option<&str>) -> Result<Response, String> {
    // The ROMBundler side module resolves symbols in the main module, and that one does not export
    // `getaddrinfo`. Calling `ToSocketAddrs` here makes the `dlopen` fail before the game opens.
    #[cfg(target_os = "emscripten")]
    {
        let _ = (url, body, delta);
        return Err(
            "HTTP networking is not available in this WebAssembly build: the main module does not export getaddrinfo"
                .to_string(),
        );
    }
    #[cfg(not(target_os = "emscripten"))]
    {
        let (machine, port, path) = split_url(url)?;
        let target = match delta {
            Some(d) if d.contains(':') => d.to_string(),
            Some(d) => format!("{d}:{port}"),
            None => format!("{machine}:{port}"),
        };
        let address = std::net::ToSocketAddrs::to_socket_addrs(&target)
            .map_err(|e| format!("could not resolve {target}: {e}"))?
            .next()
            .ok_or_else(|| format!("{target} did not resolve to any address"))?;

        let mut stream =
            TcpStream::connect_timeout(&address, TIMEOUT).map_err(|e| format!("{target}: {e}"))?;
        stream.set_read_timeout(Some(TIMEOUT)).ok();
        stream.set_write_timeout(Some(TIMEOUT)).ok();

        let header = format!(
            "POST {path} HTTP/1.1\r\nHost: {machine}\r\nContent-Type: application/octet-stream\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream
            .write_all(header.as_bytes())
            .and_then(|()| stream.write_all(body))
            .and_then(|()| stream.flush())
            .map_err(|e| format!("while sending to {target}: {e}"))?;

        let mut raw = Vec::new();
        stream
            .take(MAX_RESPONSE as u64)
            .read_to_end(&mut raw)
            .map_err(|e| format!("while reading from {target}: {e}"))?;
        split_response(&raw)
    }
}

/// Splits the HTTP response into status and body.
///
/// We interpret nothing beyond that: `Transfer-Encoding: chunked` and the like are left for when
/// they show up. With `Connection: close`, the simple server we have on the other side answers
/// with `Content-Length` and closes.
pub fn split_response(raw: &[u8]) -> Result<Response, String> {
    let end = raw
        .windows(4)
        .position(|j| j == b"\r\n\r\n")
        .ok_or_else(|| "response without end of headers".to_string())?;
    let header = String::from_utf8_lossy(&raw[..end]);
    let status = header
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| {
            format!(
                "unreadable status in {:?}",
                &header[..header.len().min(40)]
            )
        })?;
    Ok(Response {
        status,
        body: raw[end + 4..].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_url_with_and_without_port() {
        assert_eq!(
            split_url("http://www.zeeboids.com/zeeboidsService/PHP/Client/synchronize.php").unwrap(),
            (
                "www.zeeboids.com".to_string(),
                80,
                "/zeeboidsService/PHP/Client/synchronize.php".to_string()
            )
        );
        assert_eq!(
            split_url("http://127.0.0.1:8080/x").unwrap(),
            ("127.0.0.1".to_string(), 8080, "/x".to_string())
        );
        // Without a path, the root.
        assert_eq!(split_url("http://exemplo").unwrap().2, "/");
    }

    #[test]
    fn rejects_what_we_cannot_serve() {
        assert!(split_url("https://exemplo/x").is_err());
        assert!(split_url("socket://zeebo-cust.opera-mini.net:1080/").is_err());
        assert!(split_url("http://:80/x").is_err());
        assert!(split_url("http://exemplo:porta/x").is_err());
    }

    #[test]
    fn the_redirect_does_not_change_what_the_game_asked_for() {
        // The redirect is only the connection's destination. If it changed the `Host` or the
        // path, the server on the other side would receive a request different from the one the
        // game made — and its log would no longer describe the game.
        let (machine, _, path) = split_url("http://www.zeeboids.com/a/b.php").unwrap();
        assert_eq!(machine, "www.zeeboids.com");
        assert_eq!(path, "/a/b.php");
    }

    #[test]
    fn reads_status_and_body() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\n1;100001;";
        let r = split_response(raw).unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body, b"1;100001;");

        // An empty body is a legitimate response: `synchronize.php` answers like that.
        let r = split_response(b"HTTP/1.1 200 OK\r\n\r\n").unwrap();
        assert_eq!((r.status, r.body.len()), (200, 0));
        assert!(split_response(b"lixo").is_err());
    }
}
