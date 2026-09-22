// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-B2ZMVUYARSD5
/// Cellular Transport — native WebSocket relay client (desktop)
///
/// Lets the desktop client join an Android session over the internet via the
/// Cloudflare Durable Object relay (`relay.sassyconsultingllc.com`).
///
/// ## Why this is not a copy of the Android module
/// On Android, `cellular_transport.rs` is only a thread-safe queue bridge —
/// the actual WebSocket is owned by Kotlin/OkHttp, which polls/pushes the
/// queues over JNI. The desktop has no Kotlin layer, so this module owns the
/// socket natively (tokio-tungstenite) and drives the full lifecycle: token
/// fetch, dial, read/write pumps, heartbeats, and reconnect.
///
/// ## Wire compatibility with Android (the whole point)
/// The relay is a blind forwarder; it never decrypts. To interoperate with an
/// Android phone in the same room, every byte on the wire must match what the
/// phone produces:
///   - **Keying**: the QR carries a random 32-byte AES key. Both peers load it
///     as a PSK (`sassytalkie_core::crypto::CryptoSession::from_psk`). The relay
///     room id is the QR's `session_id`.
///   - **Audio frame**: `CryptoSession::encrypt(core::wire::pack_wire_frame(..))`
///     → `nonce(12) || ct || tag(16)`, sent as a single binary WS message. The
///     plaintext under the seal is the SHARED wire frame (channel/sender/name/ts
///     header + opus), byte-identical to what the Android phone pushes onto the
///     relay. RX decrypts then `unpack_wire_frame` to recover the Opus.
///   - **Control frames**: opcode `0x10..=0x1F` with a valid TLV length are
///     non-audio (heartbeat, PTT markers, partner-offline, replay). They are
///     skipped on the audio path, exactly as the Kotlin client does.
///   - **Liveness**: the relay sweeps any socket idle > 8 s. Any binary frame
///     refreshes liveness, so we emit a 2 s heartbeat (`OP_HEARTBEAT`) to stay
///     attached while idle and to show up in peers' liveness trackers.
///
/// The public surface deliberately mirrors the UDP `TransportManager`
/// (`send_audio` + `take_audio_receiver` yielding decrypted Opus) so the
/// existing TX/RX threads in `lib.rs` drive either transport with one branch.
///
/// Copyright 2025 Sassy Consulting LLC. All rights reserved.
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async_tls_with_config, Connector};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{AUTHORIZATION, HeaderValue};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tracing::{debug, info, warn};

use sassytalkie_core::crypto::CryptoSession;
use sassytalkie_core::wire;

use super::control;
use super::control_plane::{ControlAction, ControlPlane};
use super::{realtime_channel, RealtimeReceiver, RealtimeSender};

/// Relay WebSocket base (scheme + host). Path (`/ws`, `/auth`) appended per call.
pub const RELAY_WS_BASE: &str = "wss://relay.sassyconsultingllc.com";

/// Reconnect forever on transient failures, with exponential delay capped at
/// one minute. Authentication/configuration failures are terminal.
const RECONNECT_BASE_MS: u64 = 3_000;
const RECONNECT_CAP_MS: u64 = 60_000;
const RELAY_OUTBOUND_CAPACITY: usize = 32;
const RELAY_INBOUND_CAPACITY: usize = 64;
const RELAY_FRAME_MAX_AGE: Duration = Duration::from_millis(250);

/// Heartbeat cadence. Must be < the relay's 8 s staleness window so an idle
/// (not actively talking) desktop peer is never swept.
const HEARTBEAT_INTERVAL_SECS: u64 = 2;

/// Connection state, surfaced to the UI/status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CellularState {
    Disconnected = 0,
    Connecting = 1,
    Connected = 2,
    Error = 3,
}

impl CellularState {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Connecting,
            2 => Self::Connected,
            3 => Self::Error,
            _ => Self::Disconnected,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Error => "error",
        }
    }
}

/// Static configuration for a cellular session.
#[derive(Debug, Clone)]
pub struct CellularConfig {
    /// Relay room id = the QR session_id (8..=64 chars, validated by the relay).
    pub room_id: String,
    /// Human-readable device name (shown to peers; appended as `device=`).
    pub device_name: String,
    /// Stable per-install peer id (appended as `peer=`; lets the relay map this
    /// WS to a /presence row so it can skip FCM wake pushes while we're online).
    pub peer_id: String,
}

/// Native WebSocket relay client.
pub struct CellularTransport {
    config: CellularConfig,

    /// Shared session cipher (PSK from the QR). One instance serves both
    /// directions: `encrypt` advances our nonce counter; `decrypt` reads the
    /// nonce from the incoming frame and never touches our counter.
    crypto: Arc<Mutex<CryptoSession>>,

    /// Outbound: `send_audio()` pushes ready-to-send wire frames here; the
    /// write pump forwards them to the socket.
    outbound_tx: RealtimeSender<Vec<u8>>,
    outbound_rx: Mutex<Option<RealtimeReceiver<Vec<u8>>>>,

    /// Inbound: the read loop pushes DECRYPTED Opus frames here; the caller
    /// drains them via `take_audio_receiver()` (mirrors UDP transport).
    inbound_tx: RealtimeSender<super::AudioFrame>,
    inbound_rx: Mutex<Option<RealtimeReceiver<super::AudioFrame>>>,

    /// Lifecycle flag. `false` tears down the supervisor + pumps.
    running: AtomicBool,
    state: AtomicU8,

    /// Current channel, stamped into every outbound core::wire frame so the relay
    /// payload matches the Android phone byte-for-byte. Synced via `set_channel`.
    current_channel: AtomicU8,

    /// Heartbeat identity — epoch fixed per session, seq monotonic.
    session_epoch: u64,
    heartbeat_seq: AtomicU32,

    /// Stats (best-effort, relaxed).
    packets_sent: AtomicU32,
    packets_received: AtomicU32,

    /// After the first successful dial, reconnects request ?catchup=1.
    has_completed_handshake: AtomicBool,

    control: ControlPlane,
}

impl CellularTransport {
    /// Build a transport from config + the session cipher (PSK). Not connected
    /// until `connect()` is awaited.
    pub fn new(config: CellularConfig, crypto: CryptoSession, psk: [u8; 32]) -> Arc<Self> {
        let (outbound_tx, outbound_rx) =
            realtime_channel(RELAY_OUTBOUND_CAPACITY, RELAY_FRAME_MAX_AGE);
        let (inbound_tx, inbound_rx) =
            realtime_channel(RELAY_INBOUND_CAPACITY, RELAY_FRAME_MAX_AGE);
        let session_epoch = control::new_session_epoch();
        let control = ControlPlane::new();
        control.install_psk(psk, &config.room_id, &config.peer_id, session_epoch);
        Arc::new(Self {
            config,
            crypto: Arc::new(Mutex::new(crypto)),
            outbound_tx,
            outbound_rx: Mutex::new(Some(outbound_rx)),
            inbound_tx,
            inbound_rx: Mutex::new(Some(inbound_rx)),
            running: AtomicBool::new(false),
            state: AtomicU8::new(CellularState::Disconnected as u8),
            current_channel: AtomicU8::new(1),
            session_epoch,
            heartbeat_seq: AtomicU32::new(0),
            packets_sent: AtomicU32::new(0),
            packets_received: AtomicU32::new(0),
            has_completed_handshake: AtomicBool::new(false),
            control,
        })
    }

    /// Connect to the relay. Awaits the FIRST dial so auth/connect failures are
    /// surfaced to the caller, then spawns a supervisor that runs the I/O pumps
    /// and reconnects with backoff until `stop()`.
    pub async fn connect(self: &Arc<Self>) -> Result<(), String> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(()); // already running
        }
        self.set_state(CellularState::Connecting);

        let stream = match self.dial().await {
            Ok(s) => s,
            Err(e) => {
                self.set_state(CellularState::Error);
                self.running.store(false, Ordering::SeqCst);
                return Err(e.to_string());
            }
        };
        self.set_state(CellularState::Connected);
        info!("Cellular: connected to room '{}'", self.config.room_id);

        // Take the outbound receiver once; the supervisor owns it across
        // reconnects so buffered frames survive a socket drop.
        let out_rx = self
            .outbound_rx
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| "outbound receiver already taken".to_string())?;

        let me = Arc::clone(self);
        tokio::spawn(async move { me.supervise(stream, out_rx).await });
        Ok(())
    }

    /// Run the first session, then reconnect with capped backoff while running.
    async fn supervise(self: Arc<Self>, first: WsStream, mut out_rx: RealtimeReceiver<Vec<u8>>) {
        self.clone().run_io(first, &mut out_rx).await;
        self.set_state(CellularState::Disconnected);

        let mut attempt: u32 = 0;
        while self.running.load(Ordering::Relaxed) {
            attempt = attempt.saturating_add(1);
            let delay = reconnect_delay_ms(attempt);
            debug!("Cellular: reconnect attempt {} in {} ms", attempt, delay);
            tokio::time::sleep(Duration::from_millis(delay)).await;
            if !self.running.load(Ordering::Relaxed) {
                break;
            }
            self.set_state(CellularState::Connecting);
            match self.dial().await {
                Ok(stream) => {
                    attempt = 0;
                    self.set_state(CellularState::Connected);
                    info!("Cellular: reconnected to room '{}'", self.config.room_id);
                    self.clone().run_io(stream, &mut out_rx).await;
                    self.set_state(CellularState::Disconnected);
                }
                Err(e) => {
                    if e.is_terminal() {
                        self.set_state(CellularState::Error);
                        self.running.store(false, Ordering::SeqCst);
                        warn!("Cellular: terminal authentication failure: {}", e);
                        break;
                    }
                    self.set_state(CellularState::Disconnected);
                    warn!("Cellular: transient reconnect failure: {}", e);
                }
            }
        }
        self.set_state(CellularState::Disconnected);
        debug!("Cellular: supervisor exited");
    }

    /// Fetch a capability token, then open the authenticated WebSocket.
    /// Token travels in Authorization (not the query string) so it stays out
    /// of access logs; ?token= remains accepted by the worker for old clients.
    async fn dial(&self) -> Result<WsStream, DialError> {
        let token = self.fetch_token().await?;
        let mut url = format!(
            "{}/ws?room={}&device={}&peer={}&client_id={}",
            RELAY_WS_BASE,
            urlencode(&self.config.room_id),
            urlencode(&self.config.device_name),
            urlencode(&self.config.peer_id),
            uuid::Uuid::new_v4(),
        );
        if self.has_completed_handshake.load(Ordering::Relaxed) {
            url.push_str("&catchup=1");
        }
        let mut request = url
            .into_client_request()
            .map_err(|e| DialError::TerminalAuth(format!("ws request: {e}")))?;
        let bearer = format!("Bearer {token}");
        request.headers_mut().insert(
            AUTHORIZATION,
            HeaderValue::from_str(&bearer)
                .map_err(|e| DialError::TerminalAuth(format!("auth header: {e}")))?,
        );
        let tls = super::tls_pinning::client_config()
            .map_err(|e| DialError::TerminalAuth(format!("tls config: {e}")))?;
        let connector = Connector::Rustls(std::sync::Arc::new(tls));
        let (stream, _resp) = connect_async_tls_with_config(
            request,
            None,
            false,
            Some(connector),
        )
        .await
        .map_err(|e| match &e {
            WsError::Http(response) if is_terminal_http_status(response.status().as_u16()) => {
                DialError::TerminalAuth(format!("ws upgrade http {}", response.status()))
            }
            _ => DialError::Retryable(format!("ws connect failed: {}", e)),
        })?;
        self.has_completed_handshake.store(true, Ordering::Relaxed);
        Ok(stream)
    }

    /// GET the HMAC token from `/auth?room=&peer=` (required when the relay has
    /// AUTH_SECRET set, which production does). peer= is always sent when we
    /// have a peer id (this client always does).
    async fn fetch_token(&self) -> Result<String, DialError> {
        let auth_url = format!(
            "{}/auth?room={}&peer={}",
            https_base(),
            urlencode(&self.config.room_id),
            urlencode(&self.config.peer_id),
        );
        let tls = super::tls_pinning::client_config()
            .map_err(|e| DialError::Retryable(format!("tls config: {e}")))?;
        let client = reqwest::Client::builder()
            .use_preconfigured_tls(tls)
            .build()
            .map_err(|e| DialError::Retryable(format!("http client: {e}")))?;
        let resp = client
            .get(&auth_url)
            .send()
            .await
            .map_err(|e| DialError::Retryable(format!("auth request failed: {}", e)))?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            return Err(if is_terminal_http_status(status) {
                DialError::TerminalAuth(format!("auth http {}", status))
            } else {
                DialError::Retryable(format!("auth http {}", status))
            });
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| DialError::TerminalAuth(format!("auth body parse failed: {}", e)))?;
        body.get("token")
            .and_then(|t| t.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .ok_or_else(|| DialError::TerminalAuth("auth token missing".to_string()))
    }

    /// Drive one connected socket: forward outbound frames, emit heartbeats,
    /// and decrypt inbound audio. Returns when the socket closes or errors.
    async fn run_io(self: Arc<Self>, stream: WsStream, out_rx: &mut RealtimeReceiver<Vec<u8>>) {
        let (mut ws_tx, mut ws_rx) = stream.split();

        // Drop any frames that queued while disconnected — stale audio replayed
        // on reconnect is worse than a gap for a walkie.
        out_rx.discard_pending();

        let mut heartbeat = tokio::time::interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            if !self.running.load(Ordering::Relaxed) {
                let _ = ws_tx.send(Message::Close(None)).await;
                break;
            }
            tokio::select! {
                // Outbound audio (already encrypted wire frames).
                maybe_frame = out_rx.recv() => {
                    match maybe_frame {
                        Some(frame) => {
                            if ws_tx.send(Message::Binary(frame)).await.is_err() {
                                warn!("Cellular: send failed; socket dropping");
                                break;
                            }
                        }
                        None => break, // outbound channel closed — transport gone
                    }
                }
                // Heartbeat keeps us off the relay's staleness sweeper.
                _ = heartbeat.tick() => {
                    let Some(frame) = self.encode_heartbeat() else {
                        warn!("Cellular: heartbeat blocked — no authenticated control context");
                        continue;
                    };
                    if ws_tx.send(Message::Binary(frame)).await.is_err() {
                        warn!("Cellular: heartbeat send failed; socket dropping");
                        break;
                    }
                }
                // Inbound.
                maybe_msg = ws_rx.next() => {
                    match maybe_msg {
                        Some(Ok(Message::Binary(bytes))) => self.handle_inbound(bytes),
                        Some(Ok(Message::Text(t))) => debug!("Cellular control: {}", t),
                        Some(Ok(Message::Close(frame))) => {
                            info!("Cellular: relay closed connection: {:?}", frame);
                            break;
                        }
                        // Ping/Pong are auto-handled by the stream; ignore.
                        Some(Ok(_)) => {}
                        Some(Err(e)) => {
                            warn!("Cellular: read error: {}", e);
                            break;
                        }
                        None => break, // stream ended
                    }
                }
            }
        }
    }

    /// Route an inbound binary frame: authenticated control first, else audio.
    fn handle_inbound(&self, bytes: Vec<u8>) {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        match self.control.handle_inbound(&bytes, now_ms, |session| {
            *self.crypto.lock().unwrap() = session;
        }) {
            ControlAction::NotControl => {}
            ControlAction::Ignore => return,
            ControlAction::Rejected { reason, opcode } => {
                warn!(
                    "Cellular: rejected control reason={} opcode={:#04x}",
                    reason, opcode
                );
                return;
            }
            ControlAction::Heartbeat | ControlAction::Emergency | ControlAction::InstalledPq => {
                return;
            }
            ControlAction::HybridOutbound(frame) => {
                let _ = self.outbound_tx.try_send(frame);
                return;
            }
        }
        let plain = {
            let live = self.crypto.lock().unwrap().decrypt(&bytes);
            match live {
                Ok(pt) => pt,
                Err(_) => match self.control.try_decrypt_staged(&bytes) {
                    Some(pt) => {
                        if let Some(session) = self.control.promote_staged() {
                            *self.crypto.lock().unwrap() = session;
                        }
                        pt
                    }
                    None => {
                        debug!("Cellular: decrypt failed ({} bytes)", bytes.len());
                        return;
                    }
                },
            }
        };
        match wire::unpack_wire_frame(&plain) {
            Ok((_ch, _sub, sender, _name, ts, opus)) => {
                if sender == self.config.peer_id {
                    return;
                }
                self.packets_received.fetch_add(1, Ordering::Relaxed);
                let _ = self.inbound_tx.try_send(super::AudioFrame {
                    sender,
                    timestamp: ts,
                    opus,
                });
            }
            Err(e) => {
                debug!(
                    "Cellular: wire unpack failed ({} bytes): {}",
                    plain.len(),
                    e
                );
            }
        }
    }

    /// Update the channel stamped into outbound wire frames (kept in sync with
    /// the UDP transport's channel by the caller).
    pub fn set_channel(&self, channel: u8) {
        self.current_channel.store(channel, Ordering::Relaxed);
    }

    fn encode_heartbeat(&self) -> Option<Vec<u8>> {
        let seq = self.heartbeat_seq.fetch_add(1, Ordering::Relaxed);
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.control.encode_heartbeat_sealed(
            self.session_epoch,
            seq,
            now_ms,
            control::PresenceState::Idle,
            0,
        )
    }

    /// Encrypt an Opus frame and queue it for the relay. Mirrors the UDP
    /// `TransportManager::send_audio` signature so the TX thread is transport-agnostic.
    pub fn send_audio(&self, opus: &[u8]) -> Result<(), String> {
        self.send_audio_at(opus, wire::now_ms())
    }

    /// Same as [send_audio] but stamps a caller-chosen timestamp so dual-path
    /// TX (relay + UDP) can share a key for cross-transport RX dedup.
    pub fn send_audio_at(&self, opus: &[u8], timestamp: u64) -> Result<(), String> {
        if self.state() != CellularState::Connected {
            return Err("cellular not connected".to_string());
        }
        let frame = wire::pack_wire_frame(
            self.current_channel.load(Ordering::Relaxed),
            wire::SUBCH_MAIN,
            &self.config.peer_id,
            &self.config.device_name,
            timestamp,
            opus,
        );
        let sealed = self
            .crypto
            .lock()
            .unwrap()
            .encrypt(&frame)
            .map_err(|e| format!("encrypt failed: {}", e))?;
        self.outbound_tx.try_send(sealed).map_err(str::to_string)?;
        self.packets_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Hand out the decrypted-Opus receiver (once). Mirrors UDP transport.
    pub fn take_audio_receiver(&self) -> Option<RealtimeReceiver<super::AudioFrame>> {
        self.inbound_rx.lock().unwrap().take()
    }

    /// Stop the transport: tears down the supervisor + pumps on next poll.
    pub fn stop(&self) {
        info!("Cellular: stopping");
        self.running.store(false, Ordering::SeqCst);
        self.set_state(CellularState::Disconnected);
        self.control.clear();
    }

    pub fn state(&self) -> CellularState {
        CellularState::from_u8(self.state.load(Ordering::Relaxed))
    }

    pub fn room_id(&self) -> &str {
        &self.config.room_id
    }

    /// Stats as JSON (for diagnostics / status command).
    pub fn stats_json(&self) -> String {
        serde_json::json!({
            "state": self.state().as_str(),
            "room": self.config.room_id,
            "sent": self.packets_sent.load(Ordering::Relaxed),
            "received": self.packets_received.load(Ordering::Relaxed),
            "outbound_queue": self.outbound_tx.metrics(),
            "inbound_queue": self.inbound_tx.metrics(),
        })
        .to_string()
    }

    fn set_state(&self, s: CellularState) {
        self.state.store(s as u8, Ordering::Relaxed);
    }
}

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug)]
enum DialError {
    TerminalAuth(String),
    Retryable(String),
}

impl DialError {
    fn is_terminal(&self) -> bool {
        matches!(self, Self::TerminalAuth(_))
    }
}

impl std::fmt::Display for DialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TerminalAuth(message) | Self::Retryable(message) => f.write_str(message),
        }
    }
}

fn is_terminal_http_status(status: u16) -> bool {
    (400..500).contains(&status) && status != 408 && status != 429
}

fn reconnect_delay_ms(attempt: u32) -> u64 {
    let shift = attempt.saturating_sub(1).min(31);
    RECONNECT_BASE_MS
        .saturating_mul(1_u64 << shift)
        .min(RECONNECT_CAP_MS)
}

/// HTTPS base for the `/auth` fetch — same host as the WS, http(s) scheme.
fn https_base() -> String {
    RELAY_WS_BASE
        .replacen("wss://", "https://", 1)
        .replacen("ws://", "http://", 1)
}

/// Is this a non-audio control frame? Matches the relay/Kotlin classifier:
/// opcode in 0x10..=0x1F AND a TLV payload length that exactly accounts for the
/// frame size. Encrypted audio frames begin with a 12-byte random nonce, so the
/// joint opcode + exact-length check makes a false positive astronomically rare.
fn is_control_frame(b: &[u8]) -> bool {
    if b.len() < 3 {
        return false;
    }
    let op = b[0];
    if !(0x10..=0x20).contains(&op) {
        return false;
    }
    let payload_len = u16::from_le_bytes([b[1], b[2]]) as usize;
    b.len() == 3 + payload_len
}

/// Minimal percent-encoding for query values (RFC 3986 unreserved set passes
/// through; everything else is %XX). Matches the Android client's encoder.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &byte in s.as_bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sassytalkie_core::crypto::generate_psk;

    #[test]
    fn control_frames_are_detected() {
        // Real heartbeat is a TLV control frame.
        let hb = control::encode_heartbeat(0xDEADBEEF, 1, 123, control::PresenceState::Idle, 0);
        assert!(is_control_frame(&hb), "heartbeat must classify as control");

        // PTT_START_V2 TLV.
        let ptt = control::encode_ptt_start_v2(0xABCD, 7, false);
        assert!(is_control_frame(&ptt));
    }

    #[test]
    fn audio_frames_are_not_control() {
        // An encrypted audio frame: nonce(12) || ct || tag. Exercise the real
        // cipher so the first byte / length distribution is realistic.
        let key = generate_psk();
        let mut cs = CryptoSession::from_psk(&key);
        for _ in 0..1000 {
            let frame = cs.encrypt(b"twenty-ms-opus-ish-payload-bytes").unwrap();
            assert!(
                !is_control_frame(&frame),
                "encrypted audio frame misclassified as control: first byte {:#04x}, len {}",
                frame[0],
                frame.len()
            );
        }
    }

    #[test]
    fn short_frames_are_not_control() {
        assert!(!is_control_frame(&[]));
        assert!(!is_control_frame(&[0x10]));
        assert!(!is_control_frame(&[0x10, 0x00]));
    }

    #[test]
    fn psk_frame_round_trips_through_two_sessions() {
        // Proves the wire frame one peer encrypts is decryptable by another
        // peer holding the same PSK — the interop contract with Android.
        let key = generate_psk();
        let mut tx = CryptoSession::from_psk(&key);
        let rx = CryptoSession::from_psk(&key);

        let opus = b"\x01\x02\x03 pretend opus frame";
        let frame = tx.encrypt(opus).unwrap();
        assert!(!is_control_frame(&frame));
        let recovered = rx.decrypt(&frame).unwrap();
        assert_eq!(&recovered, opus);
    }

    #[test]
    fn https_base_is_derived_from_ws_base() {
        assert_eq!(https_base(), "https://relay.sassyconsultingllc.com");
    }

    #[test]
    fn urlencode_passes_unreserved_and_escapes_space() {
        assert_eq!(urlencode("Brick 2.0"), "Brick%202.0");
        assert_eq!(urlencode("abc-_.~"), "abc-_.~");
    }

    #[test]
    fn reconnect_backoff_is_exponential_capped_and_indefinite() {
        assert_eq!(reconnect_delay_ms(1), 3_000);
        assert_eq!(reconnect_delay_ms(2), 6_000);
        assert_eq!(reconnect_delay_ms(5), 48_000);
        assert_eq!(reconnect_delay_ms(6), 60_000);
        assert_eq!(reconnect_delay_ms(10_000), 60_000);
    }

    #[test]
    fn terminal_auth_statuses_do_not_retry() {
        assert!(is_terminal_http_status(400));
        assert!(is_terminal_http_status(401));
        assert!(is_terminal_http_status(403));
        assert!(!is_terminal_http_status(408));
        assert!(!is_terminal_http_status(429));
        assert!(!is_terminal_http_status(500));
    }

    /// End-to-end functional check against the LIVE production relay: two
    /// clients sharing a PSK join the same room; a frame sent by A must arrive
    /// decrypted at B. Proves the whole path — /auth token, wss handshake,
    /// relay broadcast, control-frame skipping, PSK decrypt. Ignored by default
    /// (needs network + the live relay); run with:
    ///   cargo test -p sassy-talk --lib cellular::tests::live -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "hits the live production relay; run explicitly with --ignored"]
    async fn live_two_client_loopback_through_relay() {
        use sassytalkie_core::session::SessionManager;

        // Host mints a session; joiner imports it — the real QR pairing flow.
        // Both end with the same PSK and the same room id (= session_id).
        let mut host = SessionManager::new("DesktopHost");
        let qr = host.generate_session_qr(1, 24, "LoopbackTest").unwrap();
        let host_crypto = host.get_crypto_for_channel(1).unwrap();
        let room_id = host.get_session_id(1).unwrap();

        let mut joiner = SessionManager::new("DesktopJoiner");
        let (_ch, joiner_crypto, _cohort) = joiner.import_session(&qr).unwrap();

        let host_psk: [u8; 32] = *host.get_psk_for_channel(1).unwrap();
        let joiner_psk: [u8; 32] = *joiner.get_psk_for_channel(_ch).unwrap();

        let a = CellularTransport::new(
            CellularConfig {
                room_id: room_id.clone(),
                device_name: "ClientA".into(),
                peer_id: "AAAA0001".into(),
            },
            host_crypto,
            host_psk,
        );
        let b = CellularTransport::new(
            CellularConfig {
                room_id: room_id.clone(),
                device_name: "ClientB".into(),
                peer_id: "BBBB0002".into(),
            },
            joiner_crypto,
            joiner_psk,
        );

        a.connect().await.expect("client A connect");
        b.connect().await.expect("client B connect");
        assert_eq!(a.state(), CellularState::Connected);
        assert_eq!(b.state(), CellularState::Connected);

        let mut b_rx = b.take_audio_receiver().expect("b receiver");

        // Let both sockets fully join the room before sending.
        tokio::time::sleep(Duration::from_millis(1500)).await;

        let payload = b"hello-from-A-opus-frame";
        a.send_audio(payload).expect("A send");

        let received = tokio::time::timeout(Duration::from_secs(5), b_rx.recv())
            .await
            .expect("timed out waiting for relayed frame")
            .expect("b receiver closed");

        assert_eq!(
            &received.opus, payload,
            "decrypted frame must match what A sent"
        );
        println!(
            "LIVE OK: room={} A→B {} bytes round-tripped through relay",
            room_id,
            received.opus.len()
        );

        a.stop();
        b.stop();
    }
}
