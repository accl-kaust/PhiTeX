//! The live viewer's transport (DESIGN 4.8), on `std` alone: HTTP/1.1
//! requests read off a socket, plain responses, and the WebSocket
//! protocol (RFC 6455) over the same socket after an upgrade: the
//! handshake (`Sec-WebSocket-Accept`, SHA-1 and base64 here), text frames
//! out, a client's masked frames in (fragments joined, pings answered).
//! The server is on 127.0.0.1 only, for one user's browser.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;

/// An HTTP request: its method, path (with the query), and headers (names
/// in lower case).
#[derive(Debug, Default)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
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

/// Read one request (headers at most 64 KB, a body by `Content-Length` at
/// most 16 MB). `None` at the end of the stream or on a malformed one.
#[allow(clippy::many_single_char_names)]
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
    let mut req = Request {
        method,
        path,
        headers,
        body: Vec::new(),
    };
    let len: usize = req
        .header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if len > 16 << 20 {
        return None;
    }
    req.body.resize(len, 0);
    r.read_exact(&mut req.body).ok()?;
    Some(req)
}

/// Write a whole response: `status` (`200 OK`), its content type, `body`.
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

/// The handshake's answer to a client's `Sec-WebSocket-Key`.
#[must_use]
pub fn accept_key(key: &str) -> String {
    let mut s = key.trim().as_bytes().to_vec();
    s.extend_from_slice(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    base64(&sha1(&s))
}

/// Answer an upgrade request: the socket speaks WebSocket from now on.
pub fn upgrade(w: &mut impl Write, req: &Request) -> io::Result<()> {
    let key = req.header("sec-websocket-key").unwrap_or("");
    write!(
        w,
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\r\n",
        accept_key(key)
    )?;
    w.flush()
}

/// A message read off a WebSocket.
#[derive(Debug, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
    /// The client closed (or the stream ended).
    Close,
}

/// Write a frame: final, `opcode`, unmasked (a server's).
fn write_frame(w: &mut impl Write, opcode: u8, data: &[u8]) -> io::Result<()> {
    let mut head = vec![0x80 | opcode];
    match data.len() {
        n if n < 126 => head.push(u8::try_from(n).unwrap_or(0)),
        n if n <= 0xffff => {
            head.push(126);
            head.extend_from_slice(&u16::try_from(n).unwrap_or(0).to_be_bytes());
        }
        n => {
            head.push(127);
            head.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    w.write_all(&head)?;
    w.write_all(data)?;
    w.flush()
}

/// Send `text` as a text message.
pub fn send_text(w: &mut impl Write, text: &str) -> io::Result<()> {
    write_frame(w, 1, text.as_bytes())
}

/// Read the next message (fragments joined, a ping answered on `pong`),
/// messages at most 64 MB.
pub fn read_message(r: &mut impl Read, pong: &mut dyn FnMut(&[u8])) -> io::Result<Message> {
    let mut data = Vec::new();
    let mut kind = None;
    loop {
        let mut h = [0u8; 2];
        if r.read_exact(&mut h).is_err() {
            return Ok(Message::Close);
        }
        let fin = h[0] & 0x80 != 0;
        let opcode = h[0] & 0x0f;
        let masked = h[1] & 0x80 != 0;
        let len = match h[1] & 0x7f {
            126 => {
                let mut b = [0u8; 2];
                r.read_exact(&mut b)?;
                u64::from(u16::from_be_bytes(b))
            }
            127 => {
                let mut b = [0u8; 8];
                r.read_exact(&mut b)?;
                u64::from_be_bytes(b)
            }
            n => u64::from(n),
        };
        if len > 64 << 20 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "message too long",
            ));
        }
        let mut mask = [0u8; 4];
        if masked {
            r.read_exact(&mut mask)?;
        }
        let mut payload = vec![0u8; usize::try_from(len).unwrap_or(0)];
        r.read_exact(&mut payload)?;
        if masked {
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= mask[i % 4];
            }
        }
        match opcode {
            9 => pong(&payload),
            10 => {}
            0..=2 => {
                if opcode != 0 {
                    kind = Some(opcode);
                }
                data.extend_from_slice(&payload);
                if fin {
                    return Ok(match kind {
                        Some(2) => Message::Binary(data),
                        _ => Message::Text(String::from_utf8_lossy(&data).into_owned()),
                    });
                }
            }
            _ => return Ok(Message::Close),
        }
    }
}

/// Answer a ping with `data`.
pub fn send_pong(w: &mut impl Write, data: &[u8]) -> io::Result<()> {
    write_frame(w, 10, data)
}

/// Close the connection (status 1000).
pub fn send_close(w: &mut impl Write) -> io::Result<()> {
    write_frame(w, 8, &1000u16.to_be_bytes())
}

/// SHA-1 (FIPS 180-4), for the handshake only.
#[must_use]
#[allow(clippy::many_single_char_names)]
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, x) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

/// Standard base64, padded.
#[must_use]
pub fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for (i, shift) in [18u32, 12, 6, 0].iter().enumerate() {
            if i <= c.len() {
                out.push(char::from(A[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        use std::fmt::Write as _;
        b.iter().fold(String::new(), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
    }

    #[test]
    fn sha1_vectors() {
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        let long = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(hex(&sha1(long)), "84983e441c3bd26ebaae4aa1f95129e5e54670f1");
    }

    #[test]
    fn base64_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn handshake_as_rfc6455() {
        // (RFC 6455 §1.3's example)
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn frames_round_trip() {
        // (a client's masked text frame, split in two fragments, a ping between)
        let mask = [1u8, 2, 3, 4];
        let frame = |fin: bool, op: u8, data: &[u8]| {
            let mut f = vec![
                if fin { 0x80 } else { 0 } | op,
                0x80 | u8::try_from(data.len()).unwrap(),
            ];
            f.extend_from_slice(&mask);
            f.extend(data.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
            f
        };
        let mut wire = frame(false, 1, b"hel");
        wire.extend(frame(true, 9, b"p"));
        wire.extend(frame(true, 0, b"lo"));
        let mut pings = Vec::new();
        let m = read_message(&mut &wire[..], &mut |p| pings.push(p.to_vec())).unwrap();
        assert_eq!(m, Message::Text("hello".into()));
        assert_eq!(pings, vec![b"p".to_vec()]);
        // (a server's frame: a long one with the 16-bit length)
        let mut out = Vec::new();
        send_text(&mut out, &"x".repeat(300)).unwrap();
        assert_eq!(&out[..4], &[0x81, 126, 1, 44]);
        assert_eq!(
            read_message(&mut &out[..], &mut |_| {}).unwrap(),
            Message::Text("x".repeat(300))
        );
    }
}
