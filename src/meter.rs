//! What a run sends through the fallback proxy, measured on the wire.
//!
//! A [`Meter`] listens on the loopback address and relays each connection to the proxy unchanged, counting the bytes as they pass
//! in each direction. A client given the meter's URL (see [`Meter::url`]) sends the proxy what it would send it directly: its
//! CONNECT or request lines, its Proxy-Authorization, and the TLS inside a tunnel. Nothing is parsed or rewritten, so the count is
//! what the proxy carries. Evomi bills per GB and its docs do not say which direction counts, so both are counted and kept apart:
//! `up` is written to the proxy, `down` is read from it.
//!
//! The proxy has to be an `http://` address, because the meter speaks plain TCP to it. A proxy that needs TLS is refused, rather
//! than left uncounted.

use std::{
    collections::HashMap,
    io::{self, ErrorKind, Read, Write},
    net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::SeqCst},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use url::{Host, Url};

/// How long a dial to the proxy may take.
const DIAL_TIMEOUT: Duration = Duration::from_secs(15);
/// The size of each read a relay makes.
const CHUNK: usize = 16 << 10;

/// The bytes a meter counted: `up` written to the proxy, `down` read from it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Wire {
    pub(crate) up: u64,
    pub(crate) down: u64,
}

impl Wire {
    /// The two counts added.
    pub(crate) fn plus(self, other: Wire) -> Wire {
        Wire { up: self.up + other.up, down: self.down + other.down }
    }

    /// The counts as the usage prints them.
    pub(crate) fn json(self) -> Value {
        json!({ "up": self.up, "down": self.down })
    }
}

/// The `{up, down}` count a sidecar's reply carries, when both are whole numbers. Anything else is no count.
pub(crate) fn wire_of(value: &Value) -> Option<Wire> {
    Some(Wire { up: value.get("up")?.as_u64()?, down: value.get("down")?.as_u64()? })
}

/// What the connections of one meter share.
#[derive(Default)]
struct Shared {
    up: AtomicU64,
    down: AtomicU64,
    /// Connections accepted and not yet ended.
    open: AtomicUsize,
    /// The next connection's number, for `ends`.
    next: AtomicUsize,
    /// Set once the meter takes no more connections.
    closing: AtomicBool,
    /// The client and proxy ends of each open connection, so that a meter that is finished can end them.
    ends: Mutex<HashMap<usize, (TcpStream, TcpStream)>>,
}

/// A loopback forwarder to the fallback proxy (see the module doc). A client is given its [`Meter::url`] in place of the proxy's.
pub(crate) struct Meter {
    addr: SocketAddr,
    /// The proxy's address, credential included. Only [`Meter::url`] leaves this type, and it names the loopback address.
    proxy: Url,
    shared: Arc<Shared>,
}

impl Meter {
    /// A meter for the proxy at `proxy` (`http://user:password@host:port`), listening on a loopback port.
    pub(crate) fn start(proxy: &Url) -> Result<Meter> {
        if proxy.scheme() != "http" || proxy.host().is_none() {
            bail!("the fallback proxy must be an http:// address (http://user:password@host:port)");
        }
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .map_err(|_| anyhow!("the fallback proxy's local forwarder could not listen"))?;
        let addr = listener.local_addr().map_err(|_| anyhow!("the fallback proxy's local forwarder has no address"))?;
        let shared = Arc::new(Shared::default());
        let (proxy_for_accept, shared_for_accept) = (Arc::new(proxy.clone()), Arc::clone(&shared));
        thread::spawn(move || accept(&listener, &proxy_for_accept, &shared_for_accept));
        Ok(Meter { addr, proxy: proxy.clone(), shared })
    }

    /// The URL a client is given: the proxy's, credential included, with the loopback address in place of the proxy's.
    pub(crate) fn url(&self) -> String {
        let mut url = self.proxy.clone();
        let _ = url.set_host(Some(&self.addr.ip().to_string()));
        let _ = url.set_port(Some(self.addr.port()));
        url.as_str().trim_end_matches('/').to_string()
    }

    /// What the meter has counted so far.
    pub(crate) fn wire(&self) -> Wire {
        Wire { up: self.shared.up.load(SeqCst), down: self.shared.down.load(SeqCst) }
    }

    /// Ends the meter and returns what it counted. Its open connections get `grace` to end on their own (a render's end when its
    /// process does), and the ones still open after it are closed.
    pub(crate) fn finish(self, grace: Duration) -> Wire {
        self.close(grace);
        self.wire()
    }

    fn close(&self, grace: Duration) {
        self.shared.closing.store(true, SeqCst);
        // The accept loop is blocked in accept. One connection of ours wakes it, and it sees `closing` and stops.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(200));
        let until = Instant::now() + grace;
        while self.shared.open.load(SeqCst) > 0 && Instant::now() < until {
            thread::sleep(Duration::from_millis(5));
        }
        let ends: Vec<(TcpStream, TcpStream)> = lock(&self.shared.ends).drain().map(|(_, pair)| pair).collect();
        for (client, proxy) in ends {
            let _ = client.shutdown(Shutdown::Both);
            let _ = proxy.shutdown(Shutdown::Both);
        }
    }
}

impl Drop for Meter {
    fn drop(&mut self) {
        self.close(Duration::ZERO);
    }
}

/// Accepts clients until the meter closes, and relays each on a thread of its own.
fn accept(listener: &TcpListener, proxy: &Arc<Url>, shared: &Arc<Shared>) {
    for client in listener.incoming() {
        if shared.closing.load(SeqCst) {
            break;
        }
        let Ok(client) = client else {
            thread::sleep(Duration::from_millis(10));
            continue;
        };
        shared.open.fetch_add(1, SeqCst);
        let (proxy, shared) = (Arc::clone(proxy), Arc::clone(shared));
        thread::spawn(move || serve(client, &proxy, &shared));
    }
}

/// Relays one client to the proxy until either side ends. A client whose proxy cannot be dialled is closed with nothing counted.
fn serve(client: TcpStream, proxy: &Url, shared: &Arc<Shared>) {
    if let Ok(upstream) = dial(proxy) {
        relay(client, upstream, shared);
    }
    shared.open.fetch_sub(1, SeqCst);
}

/// Relays the two ends until both are done, registering them so that a finished meter can end them.
fn relay(client: TcpStream, upstream: TcpStream, shared: &Arc<Shared>) {
    let id = shared.next.fetch_add(1, SeqCst);
    let (Ok(client_up), Ok(upstream_up)) = (client.try_clone(), upstream.try_clone()) else {
        return;
    };
    if let (Ok(client_end), Ok(upstream_end)) = (client.try_clone(), upstream.try_clone()) {
        lock(&shared.ends).insert(id, (client_end, upstream_end));
    }
    // A meter closed before the registration above has already ended its connections: this one ends too.
    if shared.closing.load(SeqCst) {
        let _ = client.shutdown(Shutdown::Both);
        let _ = upstream.shutdown(Shutdown::Both);
    }
    let shared_up = Arc::clone(shared);
    let up = thread::spawn(move || pump_up(client_up, upstream_up, &shared_up));
    pump_down(upstream, client, shared);
    let _ = up.join();
    lock(&shared.ends).remove(&id);
}

/// Copies the client's bytes to the proxy, counting each part the proxy takes. The client's end half-closes the proxy's side, and
/// an error ends both.
fn pump_up(mut client: TcpStream, mut upstream: TcpStream, shared: &Shared) {
    let mut buf = vec![0; CHUNK];
    loop {
        match client.read(&mut buf) {
            Ok(0) => {
                let _ = upstream.shutdown(Shutdown::Write);
                return;
            }
            Ok(n) => {
                if write_counted(&mut upstream, &buf[..n], &shared.up).is_err() {
                    break;
                }
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    let _ = upstream.shutdown(Shutdown::Both);
    let _ = client.shutdown(Shutdown::Both);
}

/// Copies the proxy's bytes to the client, counting each read as the proxy sent it. A client that has gone away does not stop the
/// count: the proxy sent those bytes all the same, so they are still read until the proxy ends its side or the meter closes it.
/// The proxy's end, or an error, ends both sides, so the client's side is not left half open.
fn pump_down(mut upstream: TcpStream, mut client: TcpStream, shared: &Shared) {
    let mut buf = vec![0; CHUNK];
    let mut client_open = true;
    loop {
        match upstream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                shared.down.fetch_add(n as u64, SeqCst);
                if client_open && client.write_all(&buf[..n]).is_err() {
                    client_open = false;
                }
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    let _ = upstream.shutdown(Shutdown::Both);
    let _ = client.shutdown(Shutdown::Both);
}

/// Writes all of `data` to `to`, adding each part the write takes to `count`, so that a write cut short counts what it took.
fn write_counted(to: &mut TcpStream, data: &[u8], count: &AtomicU64) -> io::Result<()> {
    let mut sent = 0;
    while sent < data.len() {
        match to.write(&data[sent..]) {
            Ok(0) => return Err(io::Error::new(ErrorKind::WriteZero, "the proxy took no bytes")),
            Ok(taken) => {
                sent += taken;
                count.fetch_add(taken as u64, SeqCst);
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// A plain TCP connection to the proxy's host and port.
fn dial(proxy: &Url) -> io::Result<TcpStream> {
    let host = match proxy.host() {
        Some(Host::Domain(name)) => name.to_string(),
        Some(Host::Ipv4(ip)) => ip.to_string(),
        Some(Host::Ipv6(ip)) => ip.to_string(),
        None => return Err(io::Error::new(ErrorKind::InvalidInput, "the proxy has no host")),
    };
    let port = proxy.port_or_known_default().unwrap_or(80);
    let mut last = io::Error::new(ErrorKind::NotFound, "the proxy's host has no address");
    for addr in (host.as_str(), port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, DIAL_TIMEOUT) {
            Ok(stream) => {
                stream.set_nodelay(true)?;
                return Ok(stream);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A proxy stand-in for tests: it echoes every connection's bytes back, and closes its side when the client does. Returns its port.
#[cfg(test)]
pub(crate) fn echo_proxy() -> u16 {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a listener");
    let port = listener.local_addr().expect("an address").port();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            thread::spawn(move || {
                let mut stream = stream;
                let mut buf = [0u8; 4096];
                while let Ok(n) = stream.read(&mut buf) {
                    if n == 0 || stream.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
                let _ = stream.shutdown(Shutdown::Write);
            });
        }
    });
    port
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).expect("a URL")
    }

    /// The address of a loopback listener that is then closed: nothing listens there.
    fn closed_port() -> u16 {
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a listener").local_addr().expect("an address").port()
    }

    #[test]
    fn relays_the_bytes_unchanged_and_counts_both_directions() {
        let port = echo_proxy();
        let meter = Meter::start(&url(&format!("http://jurl:s3cret@127.0.0.1:{port}"))).expect("a meter");
        let mut client = TcpStream::connect(meter.addr).expect("the client reaches the meter");
        let sent: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        client.write_all(&sent).expect("the client writes");
        client.shutdown(Shutdown::Write).expect("the client half-closes");
        let mut back = Vec::new();
        client.read_to_end(&mut back).expect("the echo ends when the client does");
        assert_eq!(back, sent, "the bytes come back as they went");
        assert_eq!(meter.finish(Duration::from_secs(2)), Wire { up: 10_000, down: 10_000 });
    }

    #[test]
    fn every_connection_is_counted_together() {
        let port = echo_proxy();
        let meter = Meter::start(&url(&format!("http://jurl:s3cret@127.0.0.1:{port}"))).expect("a meter");
        for _ in 0..2 {
            let mut client = TcpStream::connect(meter.addr).expect("the client reaches the meter");
            client.write_all(&[1u8; 100]).expect("the client writes");
            client.shutdown(Shutdown::Write).expect("the client half-closes");
            let mut back = Vec::new();
            client.read_to_end(&mut back).expect("the echo ends");
            assert_eq!(back.len(), 100);
        }
        assert_eq!(meter.finish(Duration::from_secs(2)), Wire { up: 200, down: 200 });
    }

    #[test]
    fn the_proxy_sees_the_credential_the_client_sent_and_the_counts_include_it() {
        // A CONNECT head with Proxy-Authorization, and the proxy's answer. The proxy records what it was sent.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a listener");
        let port = listener.local_addr().expect("an address").port();
        let (seen_tx, seen_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let answer = b"HTTP/1.1 200 Connection established\r\n\r\n";
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("the meter dials");
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if stream.read(&mut byte).expect("a head byte") == 0 {
                    break;
                }
                head.push(byte[0]);
            }
            stream.write_all(answer).expect("the answer");
            let _ = seen_tx.send(head);
            let mut rest = Vec::new();
            let _ = stream.read_to_end(&mut rest);
        });
        let meter = Meter::start(&url(&format!("http://jurl:s3cret@127.0.0.1:{port}"))).expect("a meter");
        let mut client = TcpStream::connect(meter.addr).expect("the client reaches the meter");
        let head = "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\nProxy-Authorization: Basic anVybDpzM2NyZXQ=\r\n\r\n";
        client.write_all(head.as_bytes()).expect("the client writes its head");
        let mut got = vec![0u8; answer.len()];
        client.read_exact(&mut got).expect("the answer comes back");
        client.shutdown(Shutdown::Write).expect("the client half-closes");
        let seen = seen_rx.recv_timeout(Duration::from_secs(5)).expect("the proxy saw the head");
        assert_eq!(seen, head.as_bytes(), "the proxy gets the client's head as it was sent");
        assert_eq!(got, answer);
        let wire = meter.finish(Duration::from_secs(2));
        assert_eq!(wire, Wire { up: head.len() as u64, down: answer.len() as u64 });
    }

    #[test]
    fn a_client_that_goes_away_keeps_every_byte_the_proxy_sent() {
        // The proxy sends 5000 bytes as soon as it is dialled, then waits for the client to close.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a listener");
        let port = listener.local_addr().expect("an address").port();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("the meter dials");
            stream.write_all(&[7u8; 5000]).expect("the proxy sends");
            let mut rest = Vec::new();
            let _ = stream.read_to_end(&mut rest);
        });
        let meter = Meter::start(&url(&format!("http://jurl:s3cret@127.0.0.1:{port}"))).expect("a meter");
        let mut client = TcpStream::connect(meter.addr).expect("the client reaches the meter");
        let mut got = [0u8; 1000];
        client.read_exact(&mut got).expect("the client reads a part");
        let until = Instant::now() + Duration::from_secs(5);
        while meter.wire().down < 5000 && Instant::now() < until {
            thread::sleep(Duration::from_millis(5));
        }
        drop(client);
        assert_eq!(meter.finish(Duration::from_secs(2)), Wire { up: 0, down: 5000 });
    }

    #[test]
    fn bytes_the_proxy_sends_after_the_client_has_gone_are_counted_too() {
        // The client goes first, and then the proxy sends 200 KB and closes: all of it went through the proxy, so all of it counts.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a listener");
        let port = listener.local_addr().expect("an address").port();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("the meter dials");
            thread::sleep(Duration::from_millis(300));
            let _ = stream.write_all(&[3u8; 200_000]);
        });
        let meter = Meter::start(&url(&format!("http://jurl:s3cret@127.0.0.1:{port}"))).expect("a meter");
        let client = TcpStream::connect(meter.addr).expect("the client reaches the meter");
        drop(client);
        thread::sleep(Duration::from_millis(100));
        assert_eq!(meter.finish(Duration::from_secs(5)), Wire { up: 0, down: 200_000 });
    }

    #[test]
    fn an_unreachable_proxy_closes_the_client_and_counts_nothing() {
        let meter = Meter::start(&url(&format!("http://jurl:s3cret@127.0.0.1:{}", closed_port()))).expect("a meter");
        let mut client = TcpStream::connect(meter.addr).expect("the client reaches the meter");
        client.set_read_timeout(Some(Duration::from_secs(5))).expect("a timeout");
        let mut buf = [0u8; 16];
        assert_eq!(client.read(&mut buf).expect("the client is closed, not left waiting"), 0);
        assert_eq!(meter.finish(Duration::from_secs(1)), Wire::default());
    }

    #[test]
    fn the_url_a_client_is_given_keeps_the_credential_and_names_only_the_loopback_address() {
        let meter = Meter::start(&url("http://jurl:p%40ss@proxy.example:8080")).expect("a meter");
        let given = meter.url();
        assert!(given.starts_with("http://jurl:p%40ss@127.0.0.1:"), "{given}");
        assert!(!given.contains("proxy.example"), "{given}");
        assert!(given.ends_with(&format!(":{}", meter.addr.port())), "{given}");
    }

    #[test]
    fn a_proxy_that_needs_tls_is_refused_before_anything_is_listened_for() {
        let err = Meter::start(&url("https://jurl:s3cret@proxy.example:8443")).err().expect("refused");
        assert!(format!("{err:#}").contains("http://"), "{err:#}");
    }

    #[test]
    fn a_wire_count_is_read_only_when_both_directions_are_whole_numbers() {
        assert_eq!(wire_of(&json!({ "up": 3, "down": 4 })), Some(Wire { up: 3, down: 4 }));
        assert_eq!(wire_of(&json!({ "up": 3 })), None);
        assert_eq!(wire_of(&json!({ "up": -1, "down": 4 })), None);
        assert_eq!(wire_of(&json!({ "up": "3", "down": 4 })), None);
        assert_eq!(wire_of(&Value::Null), None);
    }
}
