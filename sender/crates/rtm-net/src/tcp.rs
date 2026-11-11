//! Reconnecting TCP line writer: one background thread per target.

use crate::resolve::{order_addrs, Resolver};
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender as ChanTx, TrySendError};
use std::io::{ErrorKind, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const QUEUE_CAP: usize = 4;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
pub const BACKOFF_MIN: Duration = Duration::from_millis(250);
pub const BACKOFF_MAX: Duration = Duration::from_secs(10);
/// A packet queued while disconnected is still sent right after (re)connecting if it is
/// younger than this; older ones are simply dropped.
const PENDING_MAX_AGE: Duration = Duration::from_secs(2);
const JOIN_TIMEOUT: Duration = Duration::from_millis(500);
/// TCP keepalive: first probe after this much idle time, then every `KEEPALIVE_INTERVAL`, so
/// a viewer that was powered off (no FIN/RST) is noticed even when nothing is in flight.
pub const KEEPALIVE_TIME: Duration = Duration::from_secs(10);
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);

/// Where a TCP writer connects to.
#[derive(Clone)]
pub enum Dest {
    Addr(SocketAddr),
    /// Resolved on the worker thread before every connect attempt.
    Host {
        host: String,
        port: u16,
        resolver: Resolver,
    },
}

impl Dest {
    /// `192.168.1.50:48000` or `viewer.lan:48000`; also the dedupe key.
    pub fn label(&self) -> String {
        match self {
            Dest::Addr(a) => a.to_string(),
            Dest::Host { host, port, .. } if host.contains(':') => format!("[{host}]:{port}"),
            Dest::Host { host, port, .. } => format!("{host}:{port}"),
        }
    }
}

type Line = Arc<str>;

pub struct TcpWriter {
    dest: Dest,
    /// The address of the current / last connection (shown for host-name targets).
    peer: Arc<Mutex<Option<SocketAddr>>>,
    tx: Option<ChanTx<Line>>,
    /// Clone of the receiving end, used only to drop the oldest entry when the queue is full.
    drain: Receiver<Line>,
    connected: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    done: Receiver<()>,
    handle: Option<JoinHandle<()>>,
}

impl TcpWriter {
    pub fn spawn(dest: Dest) -> std::io::Result<Self> {
        let (tx, rx) = bounded::<Line>(QUEUE_CAP);
        let (done_tx, done) = bounded::<()>(0);
        let connected = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let peer = Arc::new(Mutex::new(None));
        let label = dest.label();
        let worker = Worker {
            dest: dest.clone(),
            label: label.clone(),
            peer: peer.clone(),
            rx: rx.clone(),
            connected: connected.clone(),
            stop: stop.clone(),
            _done: done_tx,
        };
        let handle = std::thread::Builder::new()
            .name(format!("rtm-tcp {label}"))
            .spawn(move || worker.run())?;
        Ok(Self {
            dest,
            peer,
            tx: Some(tx),
            drain: rx,
            connected,
            stop,
            done,
            handle: Some(handle),
        })
    }

    /// `TCP` target as configured, plus the resolved address for host names, e.g.
    /// `viewer.lan:48000 via 192.168.1.50:48000`.
    pub fn label(&self) -> String {
        let label = self.dest.label();
        match (
            &self.dest,
            *self.peer.lock().unwrap_or_else(|e| e.into_inner()),
        ) {
            (Dest::Host { .. }, Some(p)) => format!("{label} via {p}"),
            _ => label,
        }
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// Queue a line; never blocks. Drops the oldest queued line when full.
    pub fn push(&self, line: Line) {
        let Some(tx) = &self.tx else { return };
        let mut line = line;
        loop {
            match tx.try_send(line) {
                Ok(()) => return,
                Err(TrySendError::Full(l)) => {
                    let _ = self.drain.try_recv();
                    line = l;
                }
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
    }
}

impl TcpWriter {
    /// Ask the worker to stop without waiting (Drop then joins).
    pub fn signal_stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.tx.take(); // disconnects the worker's recv
    }
}

impl Drop for TcpWriter {
    fn drop(&mut self) {
        self.signal_stop();
        // `done` disconnects when the worker exits (its sender half is dropped).
        let exited = matches!(
            self.done.recv_timeout(JOIN_TIMEOUT),
            Err(RecvTimeoutError::Disconnected)
        );
        if let Some(h) = self.handle.take() {
            if exited {
                let _ = h.join();
            } else {
                tracing::debug!(target = %self.dest.label(), "tcp writer did not stop in time, detaching");
            }
        }
    }
}

struct Worker {
    dest: Dest,
    label: String,
    peer: Arc<Mutex<Option<SocketAddr>>>,
    rx: Receiver<Line>,
    connected: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    _done: ChanTx<()>,
}

impl Worker {
    fn run(self) {
        let mut stream: Option<TcpStream> = None;
        let mut backoff = BACKOFF_MIN;
        let mut next_attempt = Instant::now();
        let mut failures: u32 = 0;
        let mut pending: Option<(Line, Instant)> = None;

        while !self.stop.load(Ordering::Relaxed) {
            if stream.is_none() {
                let now = Instant::now();
                if now >= next_attempt {
                    match self.connect() {
                        Ok(s) => {
                            tracing::info!(target = %self.label, "tcp connected");
                            self.connected.store(true, Ordering::Relaxed);
                            failures = 0;
                            backoff = BACKOFF_MIN;
                            stream = Some(s);
                            if let Some((line, at)) = pending.take() {
                                if at.elapsed() < PENDING_MAX_AGE {
                                    self.write(&mut stream, &line, &mut pending);
                                }
                            }
                            continue;
                        }
                        Err(e) => {
                            failures += 1;
                            if failures == 1 {
                                tracing::info!(target = %self.label, error = %e, "tcp connect failed, will retry");
                            } else {
                                tracing::debug!(target = %self.label, error = %e, failures, "tcp connect failed");
                            }
                            next_attempt = Instant::now() + backoff;
                            backoff = (backoff * 2).min(BACKOFF_MAX);
                            continue;
                        }
                    }
                }
                // Wait for either the next attempt time or a packet (kept as `pending`).
                match self.rx.recv_timeout(next_attempt - now) {
                    Ok(line) => pending = Some((line, Instant::now())),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                continue;
            }

            match self.rx.recv_timeout(Duration::from_millis(250)) {
                Ok(line) => {
                    if peer_closed(stream.as_ref().expect("connected")) {
                        self.disconnected(&mut stream, "peer closed");
                        // Reconnect immediately: the peer may already be back.
                        next_attempt = Instant::now();
                        pending = Some((line, Instant::now()));
                        continue;
                    }
                    if !self.write(&mut stream, &line, &mut pending) {
                        next_attempt = Instant::now();
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    // Notice a closed peer while idle so describe() is accurate.
                    if peer_closed(stream.as_ref().expect("connected")) {
                        self.disconnected(&mut stream, "peer closed");
                        next_attempt = Instant::now();
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        if stream.is_some() {
            tracing::debug!(target = %self.label, "tcp writer stopping");
        }
        self.connected.store(false, Ordering::Relaxed);
    }

    /// Write one line; on failure drop the connection and keep the line as pending so it is
    /// retried once right after reconnecting. Returns true on success.
    fn write(
        &self,
        stream: &mut Option<TcpStream>,
        line: &Line,
        pending: &mut Option<(Line, Instant)>,
    ) -> bool {
        let s = stream.as_mut().expect("connected");
        match s.write_all(line.as_bytes()) {
            Ok(()) => true,
            Err(e) => {
                self.disconnected(stream, &e.to_string());
                *pending = Some((line.clone(), Instant::now()));
                false
            }
        }
    }

    fn disconnected(&self, stream: &mut Option<TcpStream>, why: &str) {
        stream.take();
        self.connected.store(false, Ordering::Relaxed);
        tracing::info!(target = %self.label, reason = why, "tcp disconnected, reconnecting");
    }
}

impl Worker {
    /// Connect to the address, or resolve the host name now (on this thread) and try its
    /// addresses in order, IPv4 first.
    fn connect(&self) -> std::io::Result<TcpStream> {
        let (host, port, resolver) = match &self.dest {
            Dest::Addr(a) => return connect(*a),
            Dest::Host {
                host,
                port,
                resolver,
            } => (host, *port, resolver),
        };
        let addrs = order_addrs(resolver(host, port)?);
        let mut last =
            std::io::Error::new(ErrorKind::NotFound, format!("'{host}' has no addresses"));
        for a in addrs {
            if self.stop.load(Ordering::Relaxed) {
                break;
            }
            match connect(a) {
                Ok(s) => {
                    *self.peer.lock().unwrap_or_else(|e| e.into_inner()) = Some(a);
                    return Ok(s);
                }
                Err(e) => last = std::io::Error::new(e.kind(), format!("{a}: {e}")),
            }
        }
        Err(last)
    }
}

pub(crate) fn connect(addr: SocketAddr) -> std::io::Result<TcpStream> {
    let s = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
    s.set_nodelay(true)?;
    s.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let ka = socket2::TcpKeepalive::new()
        .with_time(KEEPALIVE_TIME)
        .with_interval(KEEPALIVE_INTERVAL);
    if let Err(e) = socket2::SockRef::from(&s).set_tcp_keepalive(&ka) {
        tracing::debug!(target = %addr, error = %e, "cannot enable tcp keepalive");
    }
    Ok(s)
}

/// The viewer never sends anything, so a readable EOF (or reset) means the peer is gone.
/// Any bytes it does send are discarded.
fn peer_closed(s: &TcpStream) -> bool {
    use std::io::Read;
    if s.set_nonblocking(true).is_err() {
        return true;
    }
    let mut buf = [0u8; 512];
    let mut r: &TcpStream = s;
    let closed = loop {
        match r.read(&mut buf) {
            Ok(0) => break true,
            Ok(_) => continue,
            Err(e) if e.kind() == ErrorKind::WouldBlock => break false,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break true,
        }
    };
    if s.set_nonblocking(false).is_err() {
        return true;
    }
    closed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn connections_have_keepalive() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let s = connect(l.local_addr().unwrap()).unwrap();
        assert!(socket2::SockRef::from(&s).keepalive().unwrap());
    }

    #[test]
    fn labels() {
        let r: Resolver = Arc::new(|_, _| Ok(vec![]));
        let h = |host: &str| Dest::Host {
            host: host.into(),
            port: 48000,
            resolver: r.clone(),
        };
        assert_eq!(h("viewer.lan").label(), "viewer.lan:48000");
        assert_eq!(h("::1").label(), "[::1]:48000");
        assert_eq!(Dest::Addr("[::1]:5".parse().unwrap()).label(), "[::1]:5");
    }
}
