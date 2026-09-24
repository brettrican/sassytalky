// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-24WLN3AIQG6I
use log::{error, info, warn};
/// Cellular Transport Module — WebSocket relay via Cloudflare Durable Objects
///
/// Architecture:
///   Kotlin WebSocket client (OkHttp) connects to wss://relay.sassyconsultingllc.com/ws?room=SESSION_ID
///   (see RELAY_URL). This module is the JNI queue bridge only — the socket lives in Kotlin.
///   Binary audio frames flow through a thread-safe ring buffer between Kotlin ↔ Rust:
///
///   TX path: Rust send_audio() → outbound queue → JNI callback → Kotlin WS.send(binary)
///   RX path: Kotlin WS.onMessage(binary) → JNI push → inbound queue → Rust receive_audio()
///
/// The relay is a blind forwarder — it never decrypts. Encryption is handled
/// by TransportManager (AES-256-GCM) before data reaches this module.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sassytalkie_core::sealed;

/// Max queued packets before dropping oldest.
/// 256 packets × 20ms = 5.12 seconds of audio buffer — absorbs longer
/// cellular/relay backpressure spikes (OkHttp send retries, DO wake) without
/// the drop-oldest path that previously looked like ~30% loss under load.
const MAX_INBOUND_QUEUE_SIZE: usize = 64;
const MAX_OUTBOUND_QUEUE_SIZE: usize = 32;
const MAX_INBOUND_AGE: Duration = Duration::from_millis(1_200);
const MAX_OUTBOUND_AGE: Duration = Duration::from_millis(500);

/// Max single packet size (encrypted audio frame + overhead).
/// MUST match `wifi_transport::MAX_PACKET_SIZE` (1400): a transmitted frame is
/// mirrored to BOTH the WiFi multicast and the relay, so if this were larger a
/// 1401–1500-byte frame would reach relay peers but be silently dropped for
/// WiFi peers — asymmetric delivery. Bound by the WiFi UDP MTU, the smaller of
/// the two, since that is the path that can't fragment.
const MAX_PACKET_SIZE: usize = 1400;

/// Relay server base URL
pub const RELAY_URL: &str = "wss://relay.sassyconsultingllc.com/ws";

// ── Sealed-sender (metadata resistance) ───────────────────────────────────
// When enabled AND a sealed context (session key + stable peer id) has been
// pushed from the app, the relay connection params (room/peer/device) are
// replaced by per-epoch blinded handles from `sassytalkie_core::sealed`, so the
// relay sees only rotating opaque tokens it cannot correlate to identity or
// across time. Default OFF — opt-in and coordinated, because every member of a
// room must enable it (and share the same session key + roughly synced clock)
// to land on the same blinded room id. Old/plaintext-room peers are unaffected.
static SEALED_ENABLED: AtomicBool = AtomicBool::new(false);
// (32-byte session key, stable per-install peer id). Set from Kotlin via JNI at
// session import (the app already holds the key b64 + an InstallId). Module-
// global so we don't have to thread it through the state machine.
static SEALED_CTX: Mutex<Option<([u8; 32], String)>> = Mutex::new(None);

/// Enable/disable sealed-sender connection blinding.
pub fn set_sealed_enabled(enabled: bool) {
    SEALED_ENABLED.store(enabled, Ordering::SeqCst);
    info!("Cellular: sealed-sender enabled = {}", enabled);
}

pub fn is_sealed_enabled() -> bool {
    SEALED_ENABLED.load(Ordering::SeqCst)
}

/// Push the sealed context (session key + stable peer id) used to derive
/// blinded handles. Call after a session is established.
pub fn set_sealed_context(session_key: [u8; 32], stable_peer_id: String) {
    *SEALED_CTX.lock().unwrap_or_else(|e| e.into_inner()) = Some((session_key, stable_peer_id));
    info!(
        "Cellular: sealed context set (peer id len {})",
        SEALED_CTX
            .lock()
            .map(|g| g.as_ref().map(|(_, p)| p.len()).unwrap_or(0))
            .unwrap_or(0)
    );
}

/// Clear the sealed context (e.g. on session clear) so a stale key can't blind
/// a future room.
pub fn clear_sealed_context() {
    *SEALED_CTX.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn now_ms_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CellularState {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

/// Thread-safe packet queue for WebSocket ↔ audio pipeline
#[derive(Clone)]
pub struct PacketQueue {
    inner: Arc<Mutex<VecDeque<QueuedPacket>>>,
    max_size: usize,
    max_age: Duration,
    /// Cumulative count of frames discarded by drop-oldest overflow. Surfaced
    /// via [dropped] / get_stats() so a "garbled RX" report can be pinned to
    /// queue overflow (congestion → Opus PLC artifacts) rather than a crypto
    /// or framing fault. Lifetime-cumulative; not reset on reconnect.
    dropped: Arc<AtomicU64>,
    dropped_stale: Arc<AtomicU64>,
}

#[derive(Clone)]
struct QueuedPacket {
    data: Vec<u8>,
    queued_at: Instant,
}

impl PacketQueue {
    pub fn new(max_size: usize, max_age: Duration) -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(max_size))),
            max_size,
            max_age,
            dropped: Arc::new(AtomicU64::new(0)),
            dropped_stale: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Push a packet. Drops oldest if queue is full.
    pub fn push(&self, data: Vec<u8>) {
        self.push_at(data, Instant::now());
    }

    fn push_at(&self, data: Vec<u8>, queued_at: Instant) {
        let mut q = self.inner.lock().unwrap();
        self.drop_stale_locked(&mut q, Instant::now());
        if q.len() >= self.max_size {
            q.pop_front(); // Drop oldest to prevent unbounded growth
            self.dropped.fetch_add(1, Ordering::Relaxed);
            if self.dropped.load(Ordering::Relaxed) % 50 == 1 {
                log::warn!(
                    "PacketQueue: full ({} packets), dropping oldest (rate-limited)",
                    self.max_size
                );
            }
        }
        q.push_back(QueuedPacket { data, queued_at });
    }

    /// Cumulative number of frames discarded by drop-oldest overflow.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn dropped_stale(&self) -> u64 {
        self.dropped_stale.load(Ordering::Relaxed)
    }

    /// Returns true if queue is more than ~75% full (backpressure signal).
    /// Was 50% — too eager for dual-path skip under brief bursts.
    pub fn is_congested(&self) -> bool {
        let len = self.inner.lock().unwrap().len();
        len > self.max_size * 3 / 4
    }

    /// Pop a packet (FIFO). Returns None if empty.
    pub fn pop(&self) -> Option<Vec<u8>> {
        let mut q = self.inner.lock().unwrap();
        self.drop_stale_locked(&mut q, Instant::now());
        q.pop_front().map(|p| p.data)
    }

    /// Number of queued packets
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    /// Clear all queued packets
    pub fn clear(&self) {
        self.inner.lock().unwrap().clear();
    }

    pub fn oldest_age_ms(&self) -> i64 {
        self.inner
            .lock()
            .unwrap()
            .front()
            .map(|p| p.queued_at.elapsed().as_millis() as i64)
            .unwrap_or(-1)
    }

    fn drop_stale_locked(&self, q: &mut VecDeque<QueuedPacket>, now: Instant) {
        let mut dropped = 0;
        while q
            .front()
            .map(|p| now.saturating_duration_since(p.queued_at) > self.max_age)
            .unwrap_or(false)
        {
            q.pop_front();
            dropped += 1;
        }
        if dropped > 0 {
            self.dropped_stale.fetch_add(dropped, Ordering::Relaxed);
        }
    }
}

/// Cellular transport — bridges audio pipeline with Kotlin WebSocket client
pub struct CellularTransport {
    state: CellularState,
    room_id: String,
    device_name: String,

    /// Inbound queue: Kotlin WS.onMessage → push here → Rust receive_audio() reads
    inbound: PacketQueue,

    /// Outbound queue: Rust send_audio() writes → Kotlin polls and sends via WS
    outbound: PacketQueue,

    /// Stats
    packets_sent: u64,
    packets_send_failed: u64,
    packets_queued: u64,
    packets_received: u64,
    /// Inbound frames dropped for exceeding MAX_PACKET_SIZE (distinct from
    /// queue-overflow drops, which live on each PacketQueue).
    packets_dropped_oversize: u64,
    /// Kotlin reports the result of the actual WebSocket.send call. A failed
    /// bearer is excluded from live TX gating until reconnect or a success.
    tx_send_allowed: bool,
    last_send_success: Option<Instant>,
}

impl CellularTransport {
    pub fn new(device_name: &str) -> Self {
        Self {
            state: CellularState::Disconnected,
            room_id: String::new(),
            device_name: device_name.to_string(),
            inbound: PacketQueue::new(MAX_INBOUND_QUEUE_SIZE, MAX_INBOUND_AGE),
            outbound: PacketQueue::new(MAX_OUTBOUND_QUEUE_SIZE, MAX_OUTBOUND_AGE),
            packets_sent: 0,
            packets_send_failed: 0,
            packets_queued: 0,
            packets_received: 0,
            packets_dropped_oversize: 0,
            tx_send_allowed: false,
            last_send_success: None,
        }
    }

    /// Set the room ID (derived from QR session_id)
    pub fn set_room_id(&mut self, room_id: String) {
        info!("CellularTransport: room_id set to '{}'", room_id);
        self.room_id = room_id;
    }

    /// Get the full WebSocket URL for Kotlin to connect to.
    ///
    /// Two modes:
    ///   * Sealed-sender ON (and a sealed context is set): room/peer are
    ///     per-epoch blinded handles and the device name is suppressed to a
    ///     constant placeholder, so the relay sees only rotating opaque tokens.
    ///   * Otherwise: the classic plaintext `room` + `device` params.
    ///
    /// All values are percent-encoded regardless — the room_id originates from a
    /// scanned QR session_id (attacker-influenceable), so without encoding a
    /// crafted value containing '&'/'#' could inject or override query params.
    pub fn get_ws_url(&self) -> String {
        if is_sealed_enabled() {
            let ctx = SEALED_CTX.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((ref key, ref peer_id)) = *ctx {
                let epoch = sealed::current_epoch(now_ms_epoch(), sealed::DEFAULT_EPOCH_SECS);
                let room = sealed::blinded_room_id(key, epoch);
                let peer = sealed::sealed_peer_handle(key, peer_id, epoch);
                return format!(
                    "{}?room={}&peer={}&device={}&client_id={}",
                    RELAY_URL,
                    urlencoded(&room),
                    urlencoded(&peer),
                    urlencoded(sealed::SEALED_DEVICE_PLACEHOLDER),
                    uuid::Uuid::new_v4()
                );
            }
            // Sealed enabled but no context yet — fall through to classical so
            // the user isn't stranded off-network; the app should set the
            // context at session import before connecting.
        }
        format!(
            "{}?room={}&device={}&client_id={}",
            RELAY_URL,
            urlencoded(&self.room_id),
            urlencoded(&self.device_name),
            uuid::Uuid::new_v4()
        )
    }

    /// The blinded room id for the *next* epoch, for seamless pre-join across an
    /// epoch boundary (see `sealed::room_ids_for_handoff`). Empty when sealed is
    /// off or no context is set.
    pub fn next_epoch_room_id(&self) -> String {
        if !is_sealed_enabled() {
            return String::new();
        }
        let ctx = SEALED_CTX.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((ref key, _)) = *ctx {
            let (_, next) =
                sealed::room_ids_for_handoff(key, now_ms_epoch(), sealed::DEFAULT_EPOCH_SECS);
            next
        } else {
            String::new()
        }
    }

    /// Get current state
    pub fn get_state(&self) -> CellularState {
        self.state
    }

    /// Get the inbound queue (for JNI to push received packets)
    pub fn inbound_queue(&self) -> &PacketQueue {
        &self.inbound
    }

    /// Get the outbound queue (for JNI to poll outgoing packets)
    pub fn outbound_queue(&self) -> &PacketQueue {
        &self.outbound
    }

    // ── Called by Kotlin via JNI ──

    /// Called when Kotlin WebSocket connects successfully
    pub fn on_connected(&mut self) {
        info!(
            "CellularTransport: WebSocket connected to room '{}'",
            self.room_id
        );
        self.state = CellularState::Connected;
        self.inbound.clear();
        self.outbound.clear();
        self.packets_sent = 0;
        self.packets_send_failed = 0;
        self.packets_queued = 0;
        self.packets_received = 0;
        self.tx_send_allowed = true;
        self.last_send_success = None;
    }

    /// Called when Kotlin receives a binary message from the relay
    pub fn on_message_received(&mut self, data: Vec<u8>) {
        if data.len() > MAX_PACKET_SIZE {
            self.packets_dropped_oversize += 1;
            warn!(
                "CellularTransport: dropping oversized packet ({} bytes)",
                data.len()
            );
            return;
        }
        self.packets_received += 1;
        self.inbound.push(data);
    }

    /// Called when Kotlin WebSocket disconnects
    pub fn on_disconnected(&mut self, reason: &str) {
        info!("CellularTransport: disconnected ({})", reason);
        self.state = CellularState::Disconnected;
        self.tx_send_allowed = false;
        self.inbound.clear();
        self.outbound.clear();
    }

    /// Called when Kotlin WebSocket encounters an error
    pub fn on_error(&mut self, error: &str) {
        error!("CellularTransport: WebSocket error: {}", error);
        self.state = CellularState::Error;
        self.tx_send_allowed = false;
    }

    /// Poll outbound queue — called by Kotlin to get next packet to send
    pub fn poll_outbound(&self) -> Option<Vec<u8>> {
        self.outbound.pop()
    }

    /// Result of the real Kotlin `WebSocket.send(ByteString)` call for the
    /// packet most recently polled from Rust.
    pub fn report_send_result(&mut self, success: bool) {
        if success {
            self.packets_sent += 1;
            self.tx_send_allowed = true;
            self.last_send_success = Some(Instant::now());
            crate::diag::note_tx_success();
        } else {
            self.packets_send_failed += 1;
            self.tx_send_allowed = false;
            crate::diag::note_tx_failure();
        }
    }

    /// Live bearer state used by PTT gating. Connected alone is insufficient:
    /// after an actual send failure the relay stays non-live until reconnect or
    /// a later successful send result.
    pub fn can_transmit_realtime(&self) -> bool {
        self.state == CellularState::Connected && self.tx_send_allowed
    }

    /// Returns true if the outbound queue is more than half full.
    /// Used by TransportManager to skip dual-path sends under congestion.
    pub fn is_outbound_congested(&self) -> bool {
        self.outbound.is_congested()
    }

    // ── Called by TransportManager (audio pipeline) ──

    /// Send encrypted audio data through the WebSocket relay.
    /// Puts data into outbound queue; Kotlin picks it up and sends via WS.
    pub fn send_audio(&mut self, data: &[u8]) -> Result<usize, String> {
        if self.state != CellularState::Connected {
            return Err("Cellular transport not connected".to_string());
        }

        if data.len() > MAX_PACKET_SIZE {
            return Err(format!(
                "Packet too large: {} > {}",
                data.len(),
                MAX_PACKET_SIZE
            ));
        }

        self.outbound.push(data.to_vec());
        self.packets_queued += 1;
        Ok(data.len())
    }

    /// Receive encrypted audio data from the WebSocket relay.
    /// Reads from inbound queue (filled by Kotlin WS.onMessage).
    pub fn receive_audio(&mut self, buffer: &mut [u8]) -> Result<usize, String> {
        if self.state != CellularState::Connected {
            return Ok(0);
        }

        match self.inbound.pop() {
            Some(packet) => {
                let copy_len = packet.len().min(buffer.len());
                buffer[..copy_len].copy_from_slice(&packet[..copy_len]);
                Ok(copy_len)
            }
            None => Ok(0), // No data available — non-blocking
        }
    }

    /// Activate the transport (called after Kotlin connects)
    pub fn activate(&mut self) {
        self.state = CellularState::Connected;
    }

    /// Shutdown the transport
    pub fn shutdown(&mut self) {
        info!("CellularTransport: shutting down");
        self.state = CellularState::Disconnected;
        self.tx_send_allowed = false;
        self.inbound.clear();
        self.outbound.clear();
    }

    /// Get stats as JSON string
    pub fn get_stats(&self) -> String {
        format!(
            r#"{{"state":"{}","room":"{}","queued":{},"sent":{},"send_failed":{},"tx_live":{},"last_send_success_age_ms":{},"received":{},"inbound_queue":{},"outbound_queue":{},"inbound_oldest_age_ms":{},"outbound_oldest_age_ms":{},"dropped_inbound_overflow":{},"dropped_outbound_overflow":{},"dropped_inbound_stale":{},"dropped_outbound_stale":{},"dropped_oversize":{}}}"#,
            match self.state {
                CellularState::Disconnected => "disconnected",
                CellularState::Connecting => "connecting",
                CellularState::Connected => "connected",
                CellularState::Error => "error",
            },
            self.room_id,
            self.packets_queued,
            self.packets_sent,
            self.packets_send_failed,
            self.can_transmit_realtime(),
            self.last_send_success
                .map(|t| t.elapsed().as_millis() as i64)
                .unwrap_or(-1),
            self.packets_received,
            self.inbound.len(),
            self.outbound.len(),
            self.inbound.oldest_age_ms(),
            self.outbound.oldest_age_ms(),
            self.inbound.dropped(),
            self.outbound.dropped(),
            self.inbound.dropped_stale(),
            self.outbound.dropped_stale(),
            self.packets_dropped_oversize
        )
    }
}

/// Percent-encoding for URL query params. Encodes over UTF-8 *bytes* (not
/// chars) so multi-byte characters are escaped correctly; everything outside
/// the RFC 3986 unreserved set — including '&', '#', '%' — is escaped, which
/// is what prevents query-param injection.
fn urlencoded(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for &b in s.as_bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_drops_expired_packets_before_delivery() {
        let q = PacketQueue::new(4, Duration::from_millis(10));
        q.push_at(vec![1], Instant::now() - Duration::from_millis(20));
        q.push(vec![2]);
        assert_eq!(q.pop(), Some(vec![2]));
        assert_eq!(q.dropped_stale(), 1);
    }

    #[test]
    fn queue_is_bounded_and_drops_oldest() {
        let q = PacketQueue::new(2, Duration::from_secs(1));
        q.push(vec![1]);
        q.push(vec![2]);
        q.push(vec![3]);
        assert_eq!(q.pop(), Some(vec![2]));
        assert_eq!(q.dropped(), 1);
    }

    #[test]
    fn actual_send_failure_removes_relay_from_live_tx() {
        let mut transport = CellularTransport::new("test");
        transport.on_connected();
        assert!(transport.can_transmit_realtime());
        transport.report_send_result(false);
        assert!(!transport.can_transmit_realtime());
        transport.report_send_result(true);
        assert!(transport.can_transmit_realtime());
    }
}
