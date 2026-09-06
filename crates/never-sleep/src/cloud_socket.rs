//! The reporter owns this socket; no additional thread or power assertion.
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use never_sleep_core::{CloudIdentity, JsonStatus, Lang};
use tungstenite::client::IntoClientRequest;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use super::{heartbeat_request_json, live_status_changed};

type Socket = WebSocket<MaybeTlsStream<DeadlineStream>>;

// Socket read timeouts normally restart for every partial header/TLS record.
// Keep one deadline until the complete upgrade, then switch to nonblocking I/O.
struct DeadlineStream {
    tcp: TcpStream,
    deadline: Option<Instant>,
}

impl DeadlineStream {
    fn remaining(&self) -> io::Result<Option<Duration>> {
        self.deadline
            .map(|deadline| {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "WebSocket handshake deadline",
                    ))
                } else {
                    Ok(remaining)
                }
            })
            .transpose()
    }
}

impl Read for DeadlineStream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if let Some(remaining) = self.remaining()? {
            self.tcp.set_read_timeout(Some(remaining))?;
        }
        self.tcp.read(bytes)
    }
}

impl Write for DeadlineStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(remaining) = self.remaining()? {
            self.tcp.set_write_timeout(Some(remaining))?;
        }
        self.tcp.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.tcp.flush()
    }
}

pub(super) struct LiveSocket {
    socket: Option<Socket>,
    retry_at: Instant,
    failures: u32,
    connected_at: Instant,
    received_at: Instant,
    ping_at: Instant,
    sent: Option<(JsonStatus, Lang, Vec<String>, Instant)>,
}

impl LiveSocket {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            socket: None,
            retry_at: now,
            failures: 0,
            connected_at: now,
            received_at: now,
            ping_at: now,
            sent: None,
        }
    }

    pub fn connected(&self) -> bool {
        self.socket.is_some()
    }

    pub fn connect_if_due(&mut self, origin: &str, identity: &CloudIdentity) {
        if self.connected() || Instant::now() < self.retry_at {
            return;
        }
        match connect(origin, identity) {
            Ok(socket) => {
                self.socket = Some(socket);
                self.connected_at = Instant::now();
                self.received_at = Instant::now();
                self.ping_at = Instant::now();
                self.sent = None;
            }
            Err(_) => self.failed(),
        }
    }

    fn failed(&mut self) {
        if self.connected() && self.connected_at.elapsed() >= Duration::from_secs(60) {
            self.failures = 0;
        }
        self.socket = None;
        self.sent = None;
        self.failures = self.failures.saturating_add(1);
        // Low-frequency HTTP is the fallback. Do not repeatedly handshake an
        // old Worker or a network that blocks WebSockets.
        self.retry_at = Instant::now() + Duration::from_secs(retry_secs(self.failures));
    }

    pub fn reject(&mut self) {
        self.failed();
    }

    pub fn disconnect(&mut self) {
        if let Some(mut socket) = self.socket.take() {
            let _ = socket.close(None);
        }
        self.sent = None;
    }

    pub fn read_pending(&mut self) -> Vec<String> {
        let mut frames = Vec::new();
        let Some(socket) = self.socket.as_mut() else {
            return frames;
        };
        for _ in 0..32 {
            match socket.read() {
                Ok(Message::Text(text)) => {
                    self.received_at = Instant::now();
                    if text != "pong" {
                        frames.push(text.to_string());
                    }
                }
                Ok(Message::Ping(_) | Message::Pong(_)) => self.received_at = Instant::now(),
                Ok(Message::Close(_)) => {
                    self.failed();
                    break;
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => {
                    self.failed();
                    break;
                }
            }
        }
        frames
    }

    pub fn tick(
        &mut self,
        identity: &CloudIdentity,
        display_name: &str,
        status: &JsonStatus,
        lang: Lang,
        acks: &[String],
    ) -> Vec<String> {
        let due = self
            .sent
            .as_ref()
            .is_none_or(|(before, before_lang, before_acks, at)| {
                at.elapsed() >= Duration::from_secs(1)
                    && (live_status_changed(before, status, at.elapsed().as_secs())
                        || *before_lang != lang
                        || (!acks.is_empty() && before_acks != acks)
                        || at.elapsed() >= Duration::from_secs(300))
            });
        if due && self.connected() {
            let mut body: serde_json::Value = serde_json::from_str(&heartbeat_request_json(
                identity,
                display_name,
                status,
                lang.cloud_tag(),
                acks,
                false,
            ))
            .expect("heartbeat JSON");
            body["type"] = "status".into();
            if self.send(Message::Text(body.to_string().into())) {
                self.sent = Some((status.clone(), lang, acks.to_vec(), Instant::now()));
            }
        }
        if self.connected() && self.ping_at.elapsed() >= Duration::from_secs(10) {
            self.send(Message::Text("ping".into()));
            self.ping_at = Instant::now();
        }
        if let Some(socket) = self.socket.as_mut() {
            if let Err(e) = socket.flush() {
                if !would_block(&e) {
                    self.failed();
                }
            }
        }
        let frames = self.read_pending();
        if self.connected() && self.received_at.elapsed() >= Duration::from_secs(25) {
            self.failed();
        }
        frames
    }

    fn send(&mut self, message: Message) -> bool {
        let Some(socket) = self.socket.as_mut() else {
            return false;
        };
        match socket.send(message) {
            Ok(()) => true,
            // tungstenite retains buffered bytes until the next flush.
            Err(e) if would_block(&e) => true,
            Err(_) => {
                self.failed();
                false
            }
        }
    }
}

fn would_block(error: &tungstenite::Error) -> bool {
    matches!(error, tungstenite::Error::Io(e) if e.kind() == io::ErrorKind::WouldBlock)
}

fn retry_secs(failures: u32) -> u64 {
    (60_u64.saturating_mul(1_u64 << failures.saturating_sub(1).min(3))).min(300)
}

fn connect(origin: &str, identity: &CloudIdentity) -> Result<Socket, String> {
    let origin = origin.trim_end_matches('/');
    let origin = if let Some(rest) = origin.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = origin.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        return Err("unsupported cloud URL".into());
    };
    let mut request = format!(
        "{origin}/api/socket?device_id={}&role=mac",
        identity.device_id
    )
    .into_client_request()
    .map_err(|e| e.to_string())?;
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        format!("never-sleep-v1, auth.{}", identity.device_token)
            .parse()
            .map_err(|_| "invalid token")?,
    );
    let host = request.uri().host().ok_or("missing host")?;
    let port = request
        .uri()
        .port_u16()
        .unwrap_or(if request.uri().scheme_str() == Some("wss") {
            443
        } else {
            80
        });
    let started = Instant::now();
    let mut connected = None;
    for addr in (host, port).to_socket_addrs().map_err(|e| e.to_string())? {
        let remaining = Duration::from_secs(3).saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break;
        }
        if let Ok(stream) = TcpStream::connect_timeout(&addr, remaining) {
            connected = Some(stream);
            break;
        }
    }
    let tcp = connected.ok_or("connection failed")?;
    tcp.set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;
    tcp.set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;
    let config = tungstenite::protocol::WebSocketConfig::default()
        .write_buffer_size(0)
        .max_write_buffer_size(65536)
        .max_message_size(Some(65536))
        .max_frame_size(Some(65536));
    let stream = DeadlineStream {
        tcp,
        deadline: Some(started + Duration::from_secs(3)),
    };
    let (mut socket, _) = tungstenite::client_tls_with_config(request, stream, Some(config), None)
        .map_err(|e| e.to_string())?;
    let stream = match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => stream,
        MaybeTlsStream::Rustls(tls) => &mut tls.sock,
        _ => return Err("unsupported TLS stream".into()),
    };
    stream.deadline = None;
    stream
        .tcp
        .set_nonblocking(true)
        .map_err(|e| e.to_string())?;
    Ok(socket)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn socket_coalesces_clock_ticks_and_receives_pushed_commands() {
        use std::net::TcpListener;
        use std::sync::mpsc;
        use std::thread;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (ready_tx, ready_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut ws = tungstenite::accept_hdr(
                tcp,
                |_: &tungstenite::handshake::server::Request,
                 mut response: tungstenite::handshake::server::Response| {
                    response
                        .headers_mut()
                        .insert("Sec-WebSocket-Protocol", "never-sleep-v1".parse().unwrap());
                    Ok(response)
                },
            )
            .unwrap();
            let first: serde_json::Value =
                serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(first["type"], "status");
            ws.send(Message::Text(
                r#"{"ok":true,"commands":[{"id":"push-on","cmd":"on"}]}"#.into(),
            ))
            .unwrap();
            ready_tx.send(()).unwrap();
            // The next application message must be the ack, not a clock tick.
            let ack: serde_json::Value =
                serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(ack["ack_command_ids"][0], "push-on");
        });
        let identity = CloudIdentity {
            device_id: "a".repeat(32),
            device_token: "b".repeat(64),
        };
        let status: JsonStatus = serde_json::from_value(serde_json::json!({
            "active": true, "display":"asleep", "lid":"open", "on_ac":true,
            "battery":80, "remaining_secs":100, "elapsed_secs":10,
            "user_present":false, "stop_reason":null, "screen_off_enabled":true, "lid_awake_enabled":true
        })).unwrap();
        let mut live = LiveSocket::new();
        live.connect_if_due(&origin, &identity);
        assert!(live.connected());
        let mut frames = live.tick(&identity, "Test", &status, Lang::En, &[]);
        ready_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let mut ticking = status.clone();
        ticking.elapsed_secs = Some(11);
        ticking.remaining_secs = Some(99);
        for _ in 0..10 {
            frames.extend(live.tick(&identity, "Test", &ticking, Lang::En, &[]));
            if !frames.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(frames.iter().any(|f| f.contains("push-on")));
        live.sent.as_mut().unwrap().3 = Instant::now() - Duration::from_secs(1);
        live.tick(&identity, "Test", &ticking, Lang::En, &["push-on".into()]);
        server.join().unwrap();
    }

    #[test]
    fn slow_handshake_has_one_total_deadline() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut tcp, _) = listener.accept().unwrap();
            tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                tcp.read_exact(&mut byte).unwrap();
                bytes.push(byte[0]);
            }
            let request = String::from_utf8(bytes).unwrap();
            let key = request
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("sec-websocket-key")
                        .then_some(value.trim())
                })
                .unwrap();
            let accept = tungstenite::handshake::derive_accept_key(key.as_bytes());
            tcp.write_all(b"HTTP/1.1 101 Switching Protocols\r\n")
                .unwrap();
            thread::sleep(Duration::from_secs(2));
            let _ = tcp.write_all(b"Upgrade: websocket\r\nConnection: Upgrade\r\n");
            thread::sleep(Duration::from_secs(2));
            let _ = tcp.write_all(format!("Sec-WebSocket-Accept: {accept}\r\nSec-WebSocket-Protocol: never-sleep-v1\r\n\r\n").as_bytes());
        });
        let identity = CloudIdentity {
            device_id: "a".repeat(32),
            device_token: "b".repeat(64),
        };
        let result = connect(&origin, &identity);
        server.join().unwrap();
        assert!(
            result.is_err(),
            "partial headers must not restart the three-second handshake deadline"
        );
    }

    #[test]
    fn blocked_websockets_retry_slowly_with_a_bounded_backoff() {
        assert_eq!([1, 2, 3, 4, 100].map(retry_secs), [60, 120, 240, 300, 300]);
    }
}
