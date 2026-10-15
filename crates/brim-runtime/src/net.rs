//! Sockets for `std.net`, as the interpreter runs them. The C runtime has
//! the same functions for native executables; both report errors with the
//! same short reasons, so a program prints the same either way.

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Open sockets by handle. Handles start at 1 and are never reused.
#[derive(Debug, Default)]
pub struct Sockets {
    next: i64,
    listeners: HashMap<i64, TcpListener>,
    conns: HashMap<i64, Conn>,
}

#[derive(Debug)]
struct Conn {
    stream: TcpStream,
    /// Bytes read from the stream but not yet returned.
    buffer: Vec<u8>,
}

/// Why a network function failed, as the program sees it.
pub type NetError = String;

/// The reason for an I/O error, the same words the C runtime uses.
pub fn reason(e: &std::io::Error) -> NetError {
    match e.kind() {
        ErrorKind::ConnectionRefused => "connection refused",
        ErrorKind::AddrInUse => "address in use",
        ErrorKind::AddrNotAvailable => "address not available",
        ErrorKind::PermissionDenied => "permission denied",
        ErrorKind::TimedOut | ErrorKind::WouldBlock => "timed out",
        ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::BrokenPipe => {
            "connection reset"
        }
        ErrorKind::NotConnected => "not connected",
        _ => {
            let text = e.to_string();
            if text.contains("lookup") || text.contains("resolve") || text.contains("known") {
                "host not found"
            } else {
                "network error"
            }
        }
    }
    .to_string()
}

fn check_port(port: i64) -> Result<u16, NetError> {
    u16::try_from(port).map_err(|_| "invalid port".to_string())
}

fn addresses(host: &str, port: u16) -> Result<Vec<std::net::SocketAddr>, NetError> {
    let addrs: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|_| "host not found".to_string())?
        .collect();
    if addrs.is_empty() {
        return Err("host not found".into());
    }
    Ok(addrs)
}

const CLOSED_EARLY: &str = "connection closed before the end of the message";
const TOO_LARGE: &str = "message too large";

impl Sockets {
    fn handle(&mut self) -> i64 {
        self.next += 1;
        self.next
    }

    fn conn(&mut self, handle: i64) -> Result<&mut Conn, NetError> {
        self.conns
            .get_mut(&handle)
            .ok_or_else(|| "invalid socket".to_string())
    }

    pub fn listen(&mut self, host: &str, port: i64) -> Result<i64, NetError> {
        let port = check_port(port)?;
        let addrs = addresses(host, port)?;
        let listener = TcpListener::bind(&addrs[..]).map_err(|e| reason(&e))?;
        let handle = self.handle();
        self.listeners.insert(handle, listener);
        Ok(handle)
    }

    pub fn accept(&mut self, listener: i64) -> Result<i64, NetError> {
        let listener = self
            .listeners
            .get(&listener)
            .ok_or_else(|| "invalid socket".to_string())?;
        let (stream, _) = listener.accept().map_err(|e| reason(&e))?;
        let _ = stream.set_nodelay(true);
        let handle = self.handle();
        self.conns.insert(
            handle,
            Conn {
                stream,
                buffer: Vec::new(),
            },
        );
        Ok(handle)
    }

    pub fn connect(&mut self, host: &str, port: i64) -> Result<i64, NetError> {
        let port = check_port(port)?;
        let addrs = addresses(host, port)?;
        let stream = TcpStream::connect(&addrs[..]).map_err(|e| reason(&e))?;
        let _ = stream.set_nodelay(true);
        let handle = self.handle();
        self.conns.insert(
            handle,
            Conn {
                stream,
                buffer: Vec::new(),
            },
        );
        Ok(handle)
    }

    /// The text before `delimiter`, at most `max` bytes; the delimiter is
    /// read and dropped.
    pub fn read_until(
        &mut self,
        handle: i64,
        delimiter: &str,
        max: i64,
    ) -> Result<String, NetError> {
        if delimiter.is_empty() {
            return Err("empty delimiter".into());
        }
        let max = usize::try_from(max).map_err(|_| "invalid length".to_string())?;
        let conn = self.conn(handle)?;
        let delim = delimiter.as_bytes();
        let mut searched = 0;
        loop {
            if let Some(at) = conn.buffer[searched..]
                .windows(delim.len())
                .position(|w| w == delim)
            {
                let end = searched + at;
                if end > max {
                    return Err(TOO_LARGE.into());
                }
                let text: Vec<u8> = conn.buffer.drain(..end + delim.len()).take(end).collect();
                return String::from_utf8(text).map_err(|_| "data is not valid UTF-8".into());
            }
            searched = conn.buffer.len().saturating_sub(delim.len() - 1);
            if searched > max {
                return Err(TOO_LARGE.into());
            }
            if fill(conn)? == 0 {
                return Err(if conn.buffer.is_empty() {
                    "connection closed".into()
                } else {
                    CLOSED_EARLY.into()
                });
            }
        }
    }

    /// Exactly `count` bytes, which must be UTF-8 text.
    pub fn read_exact(&mut self, handle: i64, count: i64) -> Result<String, NetError> {
        let count = usize::try_from(count).map_err(|_| "invalid length".to_string())?;
        let conn = self.conn(handle)?;
        while conn.buffer.len() < count {
            if fill(conn)? == 0 {
                return Err(CLOSED_EARLY.into());
            }
        }
        let text: Vec<u8> = conn.buffer.drain(..count).collect();
        String::from_utf8(text).map_err(|_| "data is not valid UTF-8".into())
    }

    /// Everything until the other side closes the connection.
    pub fn read_all(&mut self, handle: i64) -> Result<String, NetError> {
        let conn = self.conn(handle)?;
        while fill(conn)? > 0 {}
        let text = std::mem::take(&mut conn.buffer);
        String::from_utf8(text).map_err(|_| "data is not valid UTF-8".into())
    }

    pub fn write(&mut self, handle: i64, text: &str) -> Result<i64, NetError> {
        let conn = self.conn(handle)?;
        conn.stream
            .write_all(text.as_bytes())
            .map_err(|e| reason(&e))?;
        Ok(text.len() as i64)
    }

    pub fn close(&mut self, handle: i64) {
        self.listeners.remove(&handle);
        if let Some(conn) = self.conns.remove(&handle) {
            let _ = conn.stream.shutdown(std::net::Shutdown::Both);
        }
    }

    pub fn local_port(&mut self, handle: i64) -> Result<i64, NetError> {
        let addr = match (self.listeners.get(&handle), self.conns.get(&handle)) {
            (Some(l), _) => l.local_addr(),
            (None, Some(c)) => c.stream.local_addr(),
            (None, None) => return Err("invalid socket".into()),
        };
        addr.map(|a| i64::from(a.port())).map_err(|e| reason(&e))
    }

    pub fn set_timeout(&mut self, handle: i64, millis: i64) -> Result<(), NetError> {
        let timeout = u64::try_from(millis).map_err(|_| "invalid timeout".to_string())?;
        let timeout = (timeout > 0).then(|| Duration::from_millis(timeout));
        let conn = self.conn(handle)?;
        conn.stream
            .set_read_timeout(timeout)
            .and_then(|()| conn.stream.set_write_timeout(timeout))
            .map_err(|e| reason(&e))
    }
}

/// Reads more bytes into the buffer; 0 at the end of the stream.
fn fill(conn: &mut Conn) -> Result<usize, NetError> {
    let mut chunk = [0u8; 8192];
    loop {
        match conn.stream.read(&mut chunk) {
            Ok(n) => {
                conn.buffer.extend_from_slice(&chunk[..n]);
                return Ok(n);
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(reason(&e)),
        }
    }
}

/// Percent-decodes `text`; with `plus`, `+` is a space too (forms and
/// query strings).
pub fn url_decode(text: &str, plus: bool) -> Result<String, NetError> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes
                    .get(i + 1..i + 3)
                    .and_then(|h| std::str::from_utf8(h).ok())
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                    .ok_or_else(|| "invalid percent escape".to_string())?;
                out.push(hex);
                i += 3;
            }
            b'+' if plus => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| "decoded text is not valid UTF-8".into())
}

/// Percent-encodes every byte but the unreserved characters.
pub fn url_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn talk_to_ourselves() {
        let mut s = Sockets::default();
        let listener = s.listen("127.0.0.1", 0).unwrap();
        let port = s.local_port(listener).unwrap();
        let client = s.connect("127.0.0.1", port).unwrap();
        let server = s.accept(listener).unwrap();
        s.write(client, "hello\r\nwörld!rest").unwrap();
        assert_eq!(
            s.read_until(server, "\r\n", 3),
            Err("message too large".to_string())
        );
        assert_eq!(s.read_until(server, "\r\n", 100).unwrap(), "hello");
        assert_eq!(s.read_exact(server, 6).unwrap(), "wörld");
        assert_eq!(s.read_until(server, "!", 100).unwrap(), "");
        s.close(client);
        assert_eq!(s.read_all(server).unwrap(), "rest");
        assert_eq!(
            s.read_until(server, "\n", 100),
            Err("connection closed".to_string())
        );
        assert_eq!(s.write(99, "x"), Err("invalid socket".to_string()));
        assert_eq!(
            s.listen("127.0.0.1", 70000),
            Err("invalid port".to_string())
        );
        s.close(server);
        s.close(listener);
    }

    #[test]
    fn urls() {
        assert_eq!(url_decode("a%20b+c%C3%A9", true).unwrap(), "a b cé");
        assert_eq!(url_decode("a+b", false).unwrap(), "a+b");
        assert_eq!(
            url_decode("%zz", true),
            Err("invalid percent escape".into())
        );
        assert_eq!(
            url_decode("%ff", true),
            Err("decoded text is not valid UTF-8".into())
        );
        assert_eq!(url_encode("a b/é~"), "a%20b%2F%C3%A9~");
    }

    #[test]
    fn errors() {
        let mut s = Sockets::default();
        // A port nothing listens on: bind one, note it, close it.
        let l = s.listen("127.0.0.1", 0).unwrap();
        let port = s.local_port(l).unwrap();
        s.close(l);
        assert_eq!(
            s.connect("127.0.0.1", port),
            Err("connection refused".to_string())
        );
        assert_eq!(
            s.connect("no-such-host.invalid", 80),
            Err("host not found".to_string())
        );
        let a = s.listen("127.0.0.1", 0).unwrap();
        let busy = s.local_port(a).unwrap();
        assert_eq!(
            s.listen("127.0.0.1", busy),
            Err("address in use".to_string())
        );
    }
}
