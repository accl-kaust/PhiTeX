//! The live viewer's transport (DESIGN 4.8): HTTP/1.1 requests read off a
//! socket and plain responses; a request to upgrade is answered here and
//! the socket handed to `tungstenite` as a WebSocket. The server is on
//! 127.0.0.1 only, for one user's browser.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;

use tungstenite::WebSocket;
use tungstenite::protocol::Role;

/// An HTTP request: its method, path (with the query), and headers (names
/// in lower case).
#[derive(Debug, Default)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
}

impl Request {
    /// The value of header `name` (lower case), if given.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// The path without its query.
    #[must_use]
    pub fn route(&self) -> &str {
        self.path.split('?').next().unwrap_or("")
    }

    /// Whether it asks to become a WebSocket.
    #[must_use]
    pub fn is_upgrade(&self) -> bool {
        self.header("upgrade")
            .is_some_and(|u| u.eq_ignore_ascii_case("websocket"))
    }
}

/// One request's line and headers (a body, if any, is read and dropped).
pub fn read_request(r: &mut BufReader<TcpStream>) -> Option<Request> {
    let mut line = String::new();
    if r.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();
    let mut headers = Vec::new();
    let mut size = 0;
    loop {
        let mut h = String::new();
        let n = r.read_line(&mut h).ok()?;
        size += n;
        if n == 0 || size > 65536 {
            return None;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        let (k, v) = h.split_once(':')?;
        headers.push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
    }
    let req = Request {
        method,
        path,
        headers,
    };
    let len: u64 = req
        .header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if len > 16 << 20 {
        return None;
    }
    io::copy(&mut r.by_ref().take(len), &mut io::sink()).ok()?;
    Some(req)
}

/// A whole response, the connection closed after it.
pub fn respond(w: &mut impl Write, status: &str, ctype: &str, body: &[u8]) -> io::Result<()> {
    write!(
        w,
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    w.write_all(body)?;
    w.flush()
}

/// The upgrade answered (RFC 6455 §4.2.2); the socket as a WebSocket, a
/// second handle on it for sending from other threads.
pub fn upgrade(
    mut conn: TcpStream,
    req: &Request,
) -> io::Result<(WebSocket<TcpStream>, WebSocket<TcpStream>)> {
    let key = req.header("sec-websocket-key").unwrap_or("");
    write!(
        conn,
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\r\n",
        tungstenite::handshake::derive_accept_key(key.trim().as_bytes())
    )?;
    conn.flush()?;
    let out = conn.try_clone()?;
    Ok((
        WebSocket::from_raw_socket(conn, Role::Server, None),
        WebSocket::from_raw_socket(out, Role::Server, None),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes() {
        let r = Request {
            path: "/t/ws?x=1".into(),
            headers: vec![("upgrade".into(), "WebSocket".into())],
            ..Request::default()
        };
        assert_eq!(r.route(), "/t/ws");
        assert!(r.is_upgrade());
    }

    #[test]
    fn handshake_as_rfc6455() {
        // (RFC 6455 §1.3's example)
        assert_eq!(
            tungstenite::handshake::derive_accept_key(b"dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }
}
