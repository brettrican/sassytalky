// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-RMF4M6XSR2QP
/// Transport Module for iOS
/// 
/// UDP multicast for WiFi-based communication
/// Same approach as desktop version

use socket2::{Domain, Protocol, Socket, Type};
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};
use thiserror::Error;

// Shared cross-platform AEAD session (AES-256-GCM, `nonce||ct||tag` wire format).
// The transport seals the WHOLE serialized packet with this — byte-for-byte the
// same layer android-native seals at — so encrypted audio interops cross-platform.
use crate::crypto::CryptoSession;

/// Multicast address
pub const MULTICAST_ADDR: &str = "239.255.42.42";

/// Multicast port
pub const MULTICAST_PORT: u16 = 5555;

/// Peer timeout (30 seconds)
const PEER_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Error, Debug)]
pub enum TransportError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    
    #[error("Failed to bind socket: {0}")]
    BindError(String),
    
    #[error("Failed to join multicast: {0}")]
    MulticastError(String),
    
    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Crypto error: {0}")]
    Crypto(String),

    #[error("Encryption required: authenticate via QR code first")]
    NotEncrypted,
}

/// Peer information
#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub device_id: u32,
    pub device_name: String,
    pub address: SocketAddr,
    pub channel: u8,
    pub last_seen: SystemTime,
}

/// Transport manager
pub struct TransportManager {
    socket: Arc<Mutex<Option<Socket>>>,
    multicast_addr: SocketAddr,
    peers: Arc<Mutex<HashMap<u32, PeerInfo>>>,
    /// Active channel encryption session. `None` until a QR session is imported
    /// (or a PSK installed); while `None`, `send` refuses and `receive` drops, so
    /// the app physically cannot move cleartext audio. Mirrors android-native's
    /// single-active-session model — the channel byte lives inside the sealed
    /// frame, so channel switching re-installs the session for the new channel.
    crypto: Arc<Mutex<Option<CryptoSession>>>,
    pending_rx: Arc<Mutex<Option<CryptoSession>>>,

    /// Relay (Cloudflare WebSocket) outbound mirror. When the relay is connected
    /// (`relay_active`), every sealed audio frame that goes out on multicast is
    /// ALSO queued here for the Swift `RelayClient` to drain and send over the WS
    /// — byte-identical to the multicast frame, exactly as android-native pushes
    /// the same sealed `payload` onto both WiFi and the cellular relay. The Swift
    /// side owns the socket; Rust just bridges frames + crypto.
    relay_active: Arc<AtomicBool>,
    relay_outbound: Arc<Mutex<VecDeque<Vec<u8>>>>,
}

/// Cap on buffered relay frames if Swift stalls draining — drop oldest (stale
/// audio replayed late is worse than a gap for a walkie).
const RELAY_OUTBOUND_CAP: usize = 256;

impl TransportManager {
    /// Create new transport manager
    pub fn new() -> Result<Self, TransportError> {
        let multicast_addr = SocketAddr::new(
            IpAddr::V4(MULTICAST_ADDR.parse().unwrap()),
            MULTICAST_PORT,
        );
        
        Ok(Self {
            socket: Arc::new(Mutex::new(None)),
            multicast_addr,
            peers: Arc::new(Mutex::new(HashMap::new())),
            crypto: Arc::new(Mutex::new(None)),
            pending_rx: Arc::new(Mutex::new(None)),
            relay_active: Arc::new(AtomicBool::new(false)),
            relay_outbound: Arc::new(Mutex::new(VecDeque::new())),
        })
    }

    /// Mark the relay connected/disconnected. While active, `send` tees every
    /// sealed frame into the relay outbound queue for Swift to forward.
    pub fn set_relay_active(&self, active: bool) {
        self.relay_active.store(active, Ordering::SeqCst);
        if !active {
            self.relay_outbound.lock().unwrap().clear();
        }
    }

    /// Drain one queued sealed frame for the Swift RelayClient to send over the
    /// WebSocket. Returns `None` when the queue is empty.
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

    /// Decrypt + return the plaintext wire frame of a sealed relay datagram
    /// received over the WebSocket. `None` if there is no session or the frame
    /// fails authentication. (The caller then `unpack_wire_frame`s it.)
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

    /// Install the active channel's encryption session. One session is active at
    /// a time; switching channels re-installs the session for the newly-selected
    /// channel (the channel byte lives INSIDE the sealed frame, so a receiver
    /// can't pick a per-channel key before decrypting — identical to Android).
    pub fn set_crypto(&self, session: CryptoSession) {
        *self.crypto.lock().unwrap() = Some(session);
        log::info!("Transport: encryption enabled");
    }

    /// Install encryption directly from a 32-byte pre-shared key.
    pub fn set_psk(&self, key: &[u8; 32]) {
        *self.crypto.lock().unwrap() = Some(CryptoSession::from_psk(key));
        log::info!("Transport: PSK encryption enabled");
    }

    /// Whether an encryption session is currently installed.
    pub fn is_encrypted(&self) -> bool {
        self.crypto.lock().unwrap().is_some()
    }

    /// Tear down the active session (disconnect / logout). After this, `send`
    /// refuses and `receive` drops until a new session is installed.
    pub fn clear_crypto(&self) {
        *self.crypto.lock().unwrap() = None;
    }
    
    /// Send a control envelope that is ALREADY sealed by `ControlAuthCodec`.
    /// Must not wrap it in the audio AEAD — that would double-encrypt and make
    /// `classify_inbound` miss `OP_AUTHENTICATED`. Used for PTT_START/STOP and
    /// hybrid frames on the LAN multicast path (relay uses `enqueue_relay_control`).
    pub fn send_control_datagram(&self, sealed: &[u8]) -> Result<(), TransportError> {
        if sealed.is_empty() {
            return Ok(());
        }
        let socket = self.socket.lock().unwrap();
        if let Some(sock) = socket.as_ref() {
            sock.send_to(sealed, &self.multicast_addr.into())?;
        }
        Ok(())
    }

    /// Receive one raw UDP datagram with no audio decrypt. The caller runs
    /// `classify_inbound` first (control plane) then `open_sealed` (audio).
    pub fn recv_datagram(&self) -> Result<Vec<u8>, TransportError> {
        let mut raw = vec![0u8; 4096];
        let socket = self.socket.lock().unwrap();
        let sock = match socket.as_ref() {
            Some(s) => s,
            None => {
                return Err(TransportError::IoError(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "Socket not initialized",
                )))
            }
        };
        let buf_uninit: &mut [std::mem::MaybeUninit<u8>] = unsafe {
            &mut *(raw.as_mut_slice() as *mut [u8] as *mut [std::mem::MaybeUninit<u8>])
        };
        match sock.recv_from(buf_uninit) {
            Ok((size, _)) => {
                raw.truncate(size);
                Ok(raw)
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => Err(TransportError::IoError(
                std::io::Error::new(std::io::ErrorKind::WouldBlock, "No data"),
            )),
            Err(e) => Err(TransportError::IoError(e)),
        }
    }

    /// Start transport
    pub fn start(&self) -> Result<(), TransportError> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        
        socket.set_reuse_address(true)?;
        socket.set_nonblocking(true)?;
        
        let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), MULTICAST_PORT);
        socket.bind(&bind_addr.into())?;
        
        // Join multicast group
        let multicast_ip: Ipv4Addr = MULTICAST_ADDR.parse().unwrap();
        socket
            .join_multicast_v4(&multicast_ip, &Ipv4Addr::UNSPECIFIED)
            .map_err(|e| TransportError::MulticastError(e.to_string()))?;
        
        *self.socket.lock().unwrap() = Some(socket);
        Ok(())
    }
    
    /// Stop transport
    pub fn stop(&self) {
        *self.socket.lock().unwrap() = None;
    }
    
    /// Send packet. SECURITY: refuses to transmit cleartext — the whole
    /// serialized packet is sealed (`nonce||ct||tag`) with the active channel's
    /// `CryptoSession`, byte-for-byte the same framing android-native produces,
    /// so encrypted audio interops cross-platform. Returns `NotEncrypted` when no
    /// session is installed (authenticate via QR first).
    pub fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        // MANDATORY ENCRYPTION. Seal first, releasing the crypto lock before
        // taking the socket lock; receive() locks in the opposite order, so
        // never holding both at once keeps the two deadlock-free.
        let payload = {
            let mut guard = self.crypto.lock().unwrap();
            match guard.as_mut() {
                Some(crypto) => crypto.encrypt(data).map_err(TransportError::Crypto)?,
                None => return Err(TransportError::NotEncrypted),
            }
        };

        // Tee the identical sealed frame to the relay (when connected) so remote
        // peers get the same bytes as local multicast peers. Mirrors android's
        // dual-path send (wifi + cellular get the same sealed payload).
        if self.relay_active.load(Ordering::SeqCst) {
            let mut q = self.relay_outbound.lock().unwrap();
            q.push_back(payload.clone());
            while q.len() > RELAY_OUTBOUND_CAP {
                q.pop_front();
            }
        }

        let socket = self.socket.lock().unwrap();
        if let Some(sock) = socket.as_ref() {
            sock.send_to(&payload, &self.multicast_addr.into())?;
        }
        Ok(())
    }

    /// Receive packet. SECURITY: drops anything that isn't a valid frame under
    /// the active session. Unencrypted, tampered, or replayed packets surface as
    /// `WouldBlock` (the RX loop treats that as "no data"), and with no session
    /// installed ALL inbound data is dropped. Mirrors android-native.
    pub fn receive(&self, buffer: &mut [u8]) -> Result<(usize, SocketAddr), TransportError> {
        // 1) Pull the raw datagram into an oversized scratch buffer — the wire
        //    frame carries a 12-byte nonce + 16-byte GCM tag over the plaintext.
        //    Scope the socket lock so it drops before we lock crypto (opposite
        //    order from send(), so the pair can't deadlock).
        let mut raw = vec![0u8; buffer.len() + 64];
        let (size, addr) = {
            let socket = self.socket.lock().unwrap();
            let sock = match socket.as_ref() {
                Some(s) => s,
                None => return Err(TransportError::IoError(
                    std::io::Error::new(std::io::ErrorKind::NotConnected, "Socket not initialized")
                )),
            };
            // socket2 0.5 takes &mut [MaybeUninit<u8>]; `raw` is already
            // initialized so the cast is sound, and recv_from writes `size`
            // valid bytes we read back from the same memory.
            let buf_uninit: &mut [std::mem::MaybeUninit<u8>] = unsafe {
                &mut *(raw.as_mut_slice() as *mut [u8] as *mut [std::mem::MaybeUninit<u8>])
            };
            match sock.recv_from(buf_uninit) {
                Ok((size, addr)) => {
                    let socket_addr = match addr.as_socket() {
                        Some(sa) => sa,
                        None => return Err(TransportError::IoError(
                            std::io::Error::new(std::io::ErrorKind::Other, "Invalid address")
                        )),
                    };
                    (size, socket_addr)
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    return Err(TransportError::IoError(
                        std::io::Error::new(std::io::ErrorKind::WouldBlock, "No data")
                    ));
                }
                Err(e) => return Err(TransportError::IoError(e)),
            }
        };

        // 2) MANDATORY DECRYPTION. Drop unencrypted / tampered / replayed frames
        //    (surfaced as WouldBlock so the RX loop just keeps polling).
        let plaintext = {
            let live = self.crypto.lock().unwrap();
            if let Some(c) = live.as_ref() {
                if let Ok(pt) = c.decrypt(&raw[..size]) {
                    drop(live);
                    pt
                } else {
                    drop(live);
                    let mut pending = self.pending_rx.lock().unwrap();
                    match pending.as_ref().and_then(|c| c.decrypt(&raw[..size]).ok()) {
                        Some(pt) => {
                            if let Some(session) = pending.take() {
                                *self.crypto.lock().unwrap() = Some(session);
                            }
                            pt
                        }
                        None => {
                            return Err(TransportError::IoError(
                                std::io::Error::new(std::io::ErrorKind::WouldBlock, "Decrypt failed — dropped")
                            ));
                        }
                    }
                }
            } else {
                drop(live);
                return Err(TransportError::IoError(
                    std::io::Error::new(std::io::ErrorKind::WouldBlock, "No session — dropped")
                ));
            }
        };
        let copy_len = plaintext.len().min(buffer.len());
        buffer[..copy_len].copy_from_slice(&plaintext[..copy_len]);
        Ok((copy_len, addr))
    }
    
    /// Add or update peer
    pub fn update_peer(&self, peer: PeerInfo) {
        let mut peers = self.peers.lock().unwrap();
        peers.insert(peer.device_id, peer);
    }
    
    /// Get active peers
    pub fn get_peers(&self) -> Vec<PeerInfo> {
        let mut peers = self.peers.lock().unwrap();
        
        // Remove stale peers
        let now = SystemTime::now();
        peers.retain(|_, peer| {
            now.duration_since(peer.last_seen).unwrap_or(Duration::MAX) < PEER_TIMEOUT
        });
        
        peers.values().cloned().collect()
    }
    
    /// Remove peer
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
