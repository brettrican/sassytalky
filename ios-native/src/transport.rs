// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-RMF4M6XSR2QP
/// Transport Module for iOS
///
/// Cloudflare WebSocket relay transport — replaces legacy UDP multicast.
/// The Swift RelayClient owns the WebSocket connection; Rust provides the
/// crypto/queue bridge via FFI:
///   - TX: `send()` seals the wire frame and enqueues for Swift to poll via
///         `sassytalkie_relay_poll_outbound()`
///   - RX: Swift calls `sassytalkie_relay_on_message(ptr, len)` with sealed
///         frames; we decrypt and unpack via `open_sealed()`

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};
use thiserror::Error;

use crate::crypto::CryptoSession;

const RELAY_OUTBOUND_CAP: usize = 64;
const PEER_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Error, Debug)]
pub enum TransportError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Crypto error: {0}")]
    Crypto(String),

    #[error("Encryption required: authenticate via QR code first")]
    NotEncrypted,
}

#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub device_id: u32,
    pub device_name: String,
    pub channel: u8,
    pub last_seen: SystemTime,
}

pub struct TransportManager {
    crypto: Arc<Mutex<Option<CryptoSession>>>,
    pending_rx: Arc<Mutex<Option<CryptoSession>>>,
    relay_active: Arc<AtomicBool>,
    relay_outbound: Arc<Mutex<VecDeque<Vec<u8>>>>,
    peers: Arc<Mutex<HashMap<u32, PeerInfo>>>,
}

impl TransportManager {
    pub fn start(&self) -> Result<(), TransportError> {
        log::info!("Transport: relay transport started");
        Ok(())
    }

    pub fn stop(&self) {
        log::info!("Transport: relay transport stopped");
    }

    pub fn new() -> Result<Self, TransportError> {
        Ok(Self {
            crypto: Arc::new(Mutex::new(None)),
            pending_rx: Arc::new(Mutex::new(None)),
            relay_active: Arc::new(AtomicBool::new(false)),
            relay_outbound: Arc::new(Mutex::new(VecDeque::new())),
            peers: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn set_relay_active(&self, active: bool) {
        self.relay_active.store(active, Ordering::SeqCst);
        if !active {
            self.relay_outbound.lock().unwrap().clear();
        }
    }

    pub fn poll_relay_outbound(&self) -> Option<Vec<u8>> {
        self.relay_outbound.lock().unwrap().pop_front()
    }

    pub fn enqueue_relay_control(&self, frame: Vec<u8>) {
        let mut q = self.relay_outbound.lock().unwrap();
        q.push_back(frame);
        while q.len() > RELAY_OUTBOUND_CAP {
            q.pop_front();
        }
    }

    pub fn open_sealed(&self, sealed: &[u8]) -> Option<Vec<u8>> {
        if let Some(pt) = self.crypto.lock().unwrap().as_ref().and_then(|c| c.decrypt(sealed).ok()) {
            return Some(pt);
        }
        let mut pending = self.pending_rx.lock().unwrap();
        if let Some(pt) = pending.as_ref().and_then(|c| c.decrypt(sealed).ok()) {
            if let Some(session) = pending.take() {
                *self.crypto.lock().unwrap() = Some(session);
            }
            return Some(pt);
        }
        None
    }

    pub fn arm_pending_rx(&self, session: CryptoSession) {
        *self.pending_rx.lock().unwrap() = Some(session);
    }

    pub fn discard_pending_rx(&self) {
        *self.pending_rx.lock().unwrap() = None;
    }

    pub fn set_crypto(&self, session: CryptoSession) {
        *self.crypto.lock().unwrap() = Some(session);
        log::info!("Transport: encryption enabled");
    }

    pub fn set_psk(&self, key: &[u8; 32]) {
        *self.crypto.lock().unwrap() = Some(CryptoSession::from_psk(key));
        log::info!("Transport: PSK encryption enabled");
    }

    pub fn is_encrypted(&self) -> bool {
        self.crypto.lock().unwrap().is_some()
    }

    pub fn clear_crypto(&self) {
        *self.crypto.lock().unwrap() = None;
    }

    pub fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        let sealed = {
            let mut crypto = self.crypto.lock().unwrap();
            crypto
                .as_mut()
                .ok_or(TransportError::NotEncrypted)?
                .encrypt(data)
                .map_err(|e| TransportError::Crypto(e))?
        };
        if self.relay_active.load(Ordering::SeqCst) {
            self.relay_outbound.lock().unwrap().push_back(sealed);
        }
        Ok(())
    }

    pub fn poll_relay_inbound(&self) -> Option<Vec<u8>> {
        None
    }

    pub fn update_peer(&self, peer: PeerInfo) {
        let mut peers = self.peers.lock().unwrap();
        peers.insert(peer.device_id, peer);
    }

    pub fn get_peers(&self) -> Vec<PeerInfo> {
        let mut peers = self.peers.lock().unwrap();
        let now = SystemTime::now();
        peers.retain(|_, peer| {
            now.duration_since(peer.last_seen).unwrap_or(Duration::MAX) < PEER_TIMEOUT
        });
        peers.values().cloned().collect()
    }

    pub fn remove_peer(&self, device_id: u32) {
        let mut peers = self.peers.lock().unwrap();
        peers.remove(&device_id);
    }
}

impl Default for TransportManager {
    fn default() -> Self {
        Self::new().expect("Failed to create transport manager")
    }
}
