//! Sockets for `std.net`, as the interpreter runs them. The C runtime has
//! the same functions for native executables; both report errors with the
//! same short reasons, so a program prints the same either way.

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Open sockets by handle. Handles start at 1 and are never reused.
///
/// Tasks share one table. It is locked only to find a socket; reading,
/// writing and waiting happen outside the lock, so a task waiting for a
/// connection does not hold up the others.
#[derive(Debug, Default)]
pub struct Sockets {
    table: Mutex<Table>,
    /// Set once a program starts a task: `next_ready` then polls instead of
    /// waiting in `accept`, because another task may keep a connection
    /// meanwhile.
    shared: AtomicBool,
    /// Set when the program ends: waits that poll give up.
    stopping: AtomicBool,
}

#[derive(Debug, Default)]
struct Table {
    next: i64,
    listeners: HashMap<i64, Arc<TcpListener>>,
    conns: HashMap<i64, Arc<Mutex<Conn>>>,
    /// Connections given back to a listener to wait for more (HTTP
    /// keep-alive): listener, connection and since when, oldest first.
    idle: Vec<(i64, i64, Instant)>,
}

/// The most connections one listener keeps waiting; more close the oldest.
pub const MAX_IDLE: usize = 256;

#[derive(Debug)]
struct Conn {
    stream: TcpStream,
    /// Bytes read from the stream but not yet returned.
    buffer: Vec<u8>,
    /// `set_timeout`: how long one read may take in all, however the data
    /// arrives, so a client sending a byte now and then cannot hold it.
    timeout: Option<Duration>,
}

enum Ready {
    Yes,
    No,
    Closed,
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

/// Locks a mutex, also after a task panicked while holding it.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Table {
    fn add(&mut self, stream: TcpStream) -> i64 {
        let _ = stream.set_nodelay(true);
        self.next += 1;
        let conn = Conn {
            stream,
            buffer: Vec::new(),
            timeout: None,
        };
        self.conns.insert(self.next, Arc::new(Mutex::new(conn)));
        self.next
    }

    fn close(&mut self, handle: i64) {
        if self.listeners.remove(&handle).is_some() {
            let kept: Vec<i64> = self
                .idle
                .iter()
                .filter(|(l, _, _)| *l == handle)
                .map(|(_, c, _)| *c)
                .collect();
            self.idle.retain(|(l, _, _)| *l != handle);
            for c in kept {
                self.close(c);
            }
        }
        self.idle.retain(|(_, c, _)| *c != handle);
        if let Some(conn) = self.conns.remove(&handle) {
            let _ = lock(&conn).stream.shutdown(std::net::Shutdown::Both);
        }
    }

    /// Whether a connection has something to read without waiting. A
    /// connection another task is using counts as not ready.
    fn readiness(&self, handle: i64) -> Ready {
        let Some(conn) = self.conns.get(&handle) else {
            return Ready::Closed;
        };
        let Ok(conn) = conn.try_lock() else {
            return Ready::No;
        };
        if !conn.buffer.is_empty() {
            return Ready::Yes;
        }
        let _ = conn.stream.set_nonblocking(true);
        let mut byte = [0u8; 1];
        let peeked = conn.stream.peek(&mut byte);
        let _ = conn.stream.set_nonblocking(false);
        match peeked {
            Ok(0) => Ready::Closed,
            Ok(_) => Ready::Yes,
            Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::Interrupted => {
                Ready::No
            }
            Err(_) => Ready::Closed,
        }
    }
}

impl Sockets {
    /// From now on, other tasks may use the sockets too.
    pub fn share(&self) {
        self.shared.store(true, Ordering::Relaxed);
    }

    /// The program is ending: waits that poll stop.
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::Relaxed);
    }

    fn conn(&self, handle: i64) -> Result<Arc<Mutex<Conn>>, NetError> {
        lock(&self.table)
            .conns
            .get(&handle)
            .cloned()
            .ok_or_else(|| "invalid socket".to_string())
    }

    fn listener(&self, handle: i64) -> Result<Arc<TcpListener>, NetError> {
        lock(&self.table)
            .listeners
            .get(&handle)
            .cloned()
            .ok_or_else(|| "invalid socket".to_string())
    }

    pub fn listen(&self, host: &str, port: i64) -> Result<i64, NetError> {
        let port = check_port(port)?;
        let addrs = addresses(host, port)?;
        let listener = TcpListener::bind(&addrs[..]).map_err(|e| reason(&e))?;
        let mut table = lock(&self.table);
        table.next += 1;
        let handle = table.next;
        table.listeners.insert(handle, Arc::new(listener));
        Ok(handle)
    }

    pub fn accept(&self, listener: i64) -> Result<i64, NetError> {
        let listener = self.listener(listener)?;
        let (stream, _) = listener.accept().map_err(|e| reason(&e))?;
        Ok(lock(&self.table).add(stream))
    }

    pub fn connect(&self, host: &str, port: i64) -> Result<i64, NetError> {
        let port = check_port(port)?;
        let addrs = addresses(host, port)?;
        let stream = TcpStream::connect(&addrs[..]).map_err(|e| reason(&e))?;
        Ok(lock(&self.table).add(stream))
    }

    /// The text before `delimiter`, at most `max` bytes; the delimiter is
    /// read and dropped.
    pub fn read_until(&self, handle: i64, delimiter: &str, max: i64) -> Result<String, NetError> {
        if delimiter.is_empty() {
            return Err("empty delimiter".into());
        }
        let max = usize::try_from(max).map_err(|_| "invalid length".to_string())?;
        let conn = self.conn(handle)?;
        let mut conn = lock(&conn);
        let deadline = conn.timeout.map(|t| Instant::now() + t);
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
            if fill(&mut conn, deadline)? == 0 {
                return Err(if conn.buffer.is_empty() {
                    "connection closed".into()
                } else {
                    CLOSED_EARLY.into()
                });
            }
        }
    }

    /// Exactly `count` bytes, which must be UTF-8 text.
    pub fn read_exact(&self, handle: i64, count: i64) -> Result<String, NetError> {
        let text = self.read_bytes(handle, count)?;
        String::from_utf8(text).map_err(|_| "data is not valid UTF-8".into())
    }

    /// Exactly `count` bytes.
    pub fn read_bytes(&self, handle: i64, count: i64) -> Result<Vec<u8>, NetError> {
        let count = usize::try_from(count).map_err(|_| "invalid length".to_string())?;
        let conn = self.conn(handle)?;
        let mut conn = lock(&conn);
        let deadline = conn.timeout.map(|t| Instant::now() + t);
        while conn.buffer.len() < count {
            if fill(&mut conn, deadline)? == 0 {
                return Err(CLOSED_EARLY.into());
            }
        }
        Ok(conn.buffer.drain(..count).collect())
    }

    pub fn write_bytes(&self, handle: i64, data: &[u8]) -> Result<i64, NetError> {
        let conn = self.conn(handle)?;
        lock(&conn).stream.write_all(data).map_err(|e| reason(&e))?;
        Ok(data.len() as i64)
    }

    /// Everything until the other side closes the connection.
    pub fn read_all(&self, handle: i64) -> Result<String, NetError> {
        let conn = self.conn(handle)?;
        let mut conn = lock(&conn);
        let deadline = conn.timeout.map(|t| Instant::now() + t);
        while fill(&mut conn, deadline)? > 0 {}
        let text = std::mem::take(&mut conn.buffer);
        String::from_utf8(text).map_err(|_| "data is not valid UTF-8".into())
    }

    pub fn write(&self, handle: i64, text: &str) -> Result<i64, NetError> {
        self.write_bytes(handle, text.as_bytes())
    }

    pub fn close(&self, handle: i64) {
        lock(&self.table).close(handle);
    }

    /// Gives `conn` back to `listener`: `next_ready` returns it when the
    /// other side sends more.
    pub fn keep(&self, listener: i64, conn: i64) {
        let mut table = lock(&self.table);
        if !table.listeners.contains_key(&listener) || !table.conns.contains_key(&conn) {
            table.close(conn);
            return;
        }
        table.idle.push((listener, conn, Instant::now()));
        let kept: Vec<usize> = (0..table.idle.len())
            .filter(|&i| table.idle[i].0 == listener)
            .collect();
        if kept.len() > MAX_IDLE {
            let (_, oldest, _) = table.idle.remove(kept[0]);
            table.close(oldest);
        }
    }

    /// The next connection of `listener` with something to read: a kept
    /// one, or a new one. Kept connections quiet for `idle_ms` are closed,
    /// and so are those the other side closed.
    pub fn next_ready(&self, listener: i64, idle_ms: i64) -> Result<i64, NetError> {
        let l = self.listener(listener)?;
        let limit = Duration::from_millis(u64::try_from(idle_ms).unwrap_or(0));
        loop {
            {
                let mut table = lock(&self.table);
                let now = Instant::now();
                let mut i = 0;
                while i < table.idle.len() {
                    let (owner, c, since) = table.idle[i];
                    if owner != listener {
                        i += 1;
                        continue;
                    }
                    match table.readiness(c) {
                        Ready::Yes => {
                            table.idle.remove(i);
                            return Ok(c);
                        }
                        Ready::Closed => {
                            table.idle.remove(i);
                            table.close(c);
                        }
                        Ready::No if now.duration_since(since) >= limit => {
                            table.idle.remove(i);
                            table.close(c);
                        }
                        Ready::No => i += 1,
                    }
                }
                let waiting = table.idle.iter().any(|(owner, _, _)| *owner == listener);
                // Alone, with nothing kept, a program can wait in `accept`.
                if !waiting && !self.shared.load(Ordering::Relaxed) {
                    drop(table);
                    return self.accept(listener);
                }
            }
            if self.stopping.load(Ordering::Relaxed) {
                return Err("stopped".into());
            }
            let _ = l.set_nonblocking(true);
            let accepted = l.accept();
            let _ = l.set_nonblocking(false);
            match accepted {
                Ok((stream, _)) => {
                    // An accepted socket can inherit non-blocking mode.
                    let _ = stream.set_nonblocking(false);
                    return Ok(lock(&self.table).add(stream));
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(e) => return Err(reason(&e)),
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    pub fn local_port(&self, handle: i64) -> Result<i64, NetError> {
        let addr = {
            let table = lock(&self.table);
            match (table.listeners.get(&handle), table.conns.get(&handle)) {
                (Some(l), _) => l.local_addr(),
                (None, Some(c)) => lock(c).stream.local_addr(),
                (None, None) => return Err("invalid socket".into()),
            }
        };
        addr.map(|a| i64::from(a.port())).map_err(|e| reason(&e))
    }

    pub fn set_timeout(&self, handle: i64, millis: i64) -> Result<(), NetError> {
        let timeout = u64::try_from(millis).map_err(|_| "invalid timeout".to_string())?;
        let timeout = (timeout > 0).then(|| Duration::from_millis(timeout));
        let conn = self.conn(handle)?;
        let mut conn = lock(&conn);
        conn.timeout = timeout;
        conn.stream
            .set_read_timeout(timeout)
            .and_then(|()| conn.stream.set_write_timeout(timeout))
            .map_err(|e| reason(&e))
    }
}

/// Reads more bytes into the buffer; 0 at the end of the stream.
fn fill(conn: &mut Conn, deadline: Option<Instant>) -> Result<usize, NetError> {
    if let Some(deadline) = deadline {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("timed out".into());
        }
        let _ = conn.stream.set_read_timeout(Some(left));
    }
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
        let s = Sockets::default();
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
        let s = Sockets::default();
        // Port 1 has no listener; a port just closed could be taken by a
        // test running in parallel.
        assert_eq!(
            s.connect("127.0.0.1", 1),
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
