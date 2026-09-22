// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-G67R6OEGM2HO
// Sassy-Talk Core Library
// Copyright 2025 Sassy Consulting LLC. All rights reserved.

//! # Sassy-Talk
//!
//! Cross-platform PTT walkie-talkie with retro vibes.
//!
//! ## Supported Platforms
//! - Windows (10+)
//! - macOS (11.0+)
//! - Linux (Ubuntu 22.04+)
//!
//! ## Architecture
//! ```text
//! [Mic] → [CPAL] → [Opus] → [AES-GCM] → [UDP Multicast] → [Decrypt] → [Opus] → [Speaker]
//! ```
//!
//! ## Transport Strategy
//! - WiFi UDP Multicast: Primary transport (works everywhere)
//! - Auto-discovery via multicast beacons
//! - No pairing required

pub mod audio;
pub mod codec;
pub mod commands;
pub mod constants;
pub mod protocol;
pub mod security;
pub mod tones;
pub mod transport;

// Re-exports
pub use audio::{AudioDeviceInfo, AudioEngine, AudioState};
pub use codec::{AudioFrame, OpusDecoder, OpusEncoder, FRAME_SIZE, SAMPLE_RATE};
pub use protocol::{Packet, PacketType};
pub use security::CryptoEngine;
pub use tones::{ToneError, TonePlayer, ToneType};
pub use transport::{PeerInfo, TransportConfig, TransportManager};

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

// Use centralized version constant
pub use constants::VERSION;

/// Application error types
#[derive(Error, Debug)]
pub enum AppError {
    #[error("Audio error: {0}")]
    AudioError(#[from] audio::AudioError),

    #[error("Codec error: {0}")]
    CodecError(#[from] codec::CodecError),

    #[error("Transport error: {0}")]
    TransportError(#[from] transport::TransportError),

    #[error("Already transmitting")]
    AlreadyTransmitting,

    #[error("Not connected")]
    NotConnected,

    #[error("Not transmitting")]
    NotTransmitting,
}

/// Connection status
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub enum ConnectionStatus {
    Disconnected,
    Discovering,
    Connected,
    Transmitting,
    Receiving,
}

/// Send one Opus frame on every live plane so mixed-protocol peers hear us.
/// XOR-routing (relay OR UDP) left LAN-only and relay-only peers deaf to each other.
async fn send_opus_fanout(
    opus_data: &[u8],
    cellular: &Option<Arc<transport::CellularTransport>>,
    transport: &Arc<Mutex<TransportManager>>,
) {
    let ts = sassytalkie_core::wire::now_ms();
    let mut relay_ok = false;
    if let Some(cell) = cellular {
        match cell.send_audio_at(opus_data, ts) {
            Ok(()) => relay_ok = true,
            Err(e) => error!("Failed to send audio (cellular): {}", e),
        }
    }
    let transport_lock = transport.lock().await;
    if let Err(e) = transport_lock.send_audio_at(opus_data, Some(ts)) {
        if relay_ok {
            warn!("UDP send failed while relay is up: {}", e);
        } else {
            error!("Failed to send audio: {}", e);
        }
    }
}

/// Application state
pub struct AppState {
    // Device info
    device_id: u32,
    device_name: String,

    // Core engines
    audio: Arc<Mutex<AudioEngine>>,
    transport: Arc<Mutex<TransportManager>>,
    tone_player: Arc<TonePlayer>,

    // Channel
    current_channel: Arc<AtomicU8>,

    // Status
    connection_status: Arc<RwLock<ConnectionStatus>>,
    is_transmitting: Arc<AtomicBool>,
    is_receiving: Arc<AtomicBool>,

    // PTT threads
    tx_thread: Arc<Mutex<Option<JoinHandle<()>>>>,
    rx_thread: Arc<Mutex<Option<JoinHandle<()>>>>,

    // Cellular relay session (internet transport via the Cloudflare relay).
    // None when not joined. When Some, PTT TX fans out here AND UDP multicast
    // so mixed-protocol peers can hear each other. A dedicated RX loop feeds
    // decrypted Opus into the shared audio pipeline.
    cellular: Arc<Mutex<Option<Arc<transport::CellularTransport>>>>,
    cellular_rx_thread: Arc<Mutex<Option<JoinHandle<()>>>>,

    // ONE AudioCache shared by every RX loop. The core cache's cross-transport
    // dedup keys on (sender_id, timestamp) and only works if all RX paths
    // converge on the same instance — a private cache per loop meant a phone
    // dual-sending on WiFi multicast + relay was played twice here.
    audio_cache: Arc<Mutex<sassytalkie_core::audio_cache::AudioCache>>,

    // Settings
    roger_beep: Arc<AtomicBool>,
    vox_enabled: Arc<AtomicBool>,
    vox_threshold: Arc<RwLock<f32>>,
}

impl AppState {
    /// Create new application state
    pub fn new(device_id: u32, device_name: String) -> Self {
        info!("Creating AppState");
        info!("Device ID: {:08X}", device_id);
        info!("Device Name: {}", device_name);

        let audio = Arc::new(Mutex::new(
            AudioEngine::new().expect("Failed to create audio engine"),
        ));

        let transport = Arc::new(Mutex::new(
            TransportManager::new(device_id, device_name.clone())
                .expect("Failed to create transport manager"),
        ));

        Self {
            device_id,
            device_name,
            audio,
            transport,
            tone_player: Arc::new(TonePlayer::new()),
            current_channel: Arc::new(AtomicU8::new(1)),
            connection_status: Arc::new(RwLock::new(ConnectionStatus::Disconnected)),
            is_transmitting: Arc::new(AtomicBool::new(false)),
            is_receiving: Arc::new(AtomicBool::new(false)),
            tx_thread: Arc::new(Mutex::new(None)),
            rx_thread: Arc::new(Mutex::new(None)),
            cellular: Arc::new(Mutex::new(None)),
            cellular_rx_thread: Arc::new(Mutex::new(None)),
            audio_cache: Arc::new(Mutex::new({
                let mut cache = sassytalkie_core::audio_cache::AudioCache::new();
                // Mix mode off by default — preserves classic walkie-talkie
                // semantics (sequential utterances on overlap).
                cache.set_mix_mode_enabled(false);
                cache
            })),
            roger_beep: Arc::new(AtomicBool::new(true)),
            vox_enabled: Arc::new(AtomicBool::new(false)),
            vox_threshold: Arc::new(RwLock::new(0.1)),
        }
    }

    /// Start discovery and receiving
    pub async fn start_discovery(&self) -> Result<(), AppError> {
        info!("Starting discovery");

        // Start transport
        let transport = self.transport.lock().await;
        transport.start().await?;

        *self.connection_status.write().await = ConnectionStatus::Discovering;

        // Start RX thread
        self.start_rx_thread().await?;

        *self.connection_status.write().await = ConnectionStatus::Connected;

        // Play connection success tone (3-tone chime) - use spawn_blocking since Stream is !Send
        let tone_player = Arc::clone(&self.tone_player);
        tokio::task::spawn_blocking(move || {
            if let Err(e) = tone_player.play_sync(ToneType::ConnectionSuccess) {
                warn!("Failed to play connection tone: {}", e);
            }
        });

        Ok(())
    }

    /// Stop discovery
    pub async fn stop_discovery(&self) -> Result<(), AppError> {
        info!("Stopping discovery");

        // Stop transport
        let transport = self.transport.lock().await;
        transport.stop();
        drop(transport);

        // Stop RX thread
        self.stop_rx_thread().await;

        // Tear down any active cellular relay session too — otherwise its
        // WebSocket + RX task leak across stop/disconnect (the Tauri
        // `disconnect` command routes here).
        self.leave_cellular().await;

        *self.connection_status.write().await = ConnectionStatus::Disconnected;

        Ok(())
    }

    /// Join a cellular relay session from a scanned/pasted QR JSON payload.
    ///
    /// Imports the session through the shared `sassytalkie_core::session`
    /// SessionManager so the PSK and relay room id are derived identically to
    /// Android — that's what makes the desktop wire-compatible. Returns the
    /// relay room id (= QR session_id) on success.
    pub async fn join_cellular(&self, qr_json: &str) -> Result<String, String> {
        // Derive (channel, PSK CryptoSession, _cohort) + room id from the QR.
        let mut sm = sassytalkie_core::session::SessionManager::new(&self.device_name);
        let (channel, crypto, _cohort) = sm.import_session(qr_json)?;
        let room_id = sm
            .get_session_id(channel)
            .ok_or_else(|| "imported session has no session_id".to_string())?;

        let psk: [u8; 32] = *sm
            .get_psk_for_channel(channel)
            .ok_or_else(|| "imported session has no room secret".to_string())?;
        if !sassytalkie_core::enrollment::join_authorized(&room_id, Some(&psk), None, None) {
            return Err("enrollment rejected: room id is not authorization".to_string());
        }
        self.transport.lock().await.set_session_psk(&psk);
        self.current_channel.store(channel, Ordering::Relaxed);
        self.transport.lock().await.set_channel(channel);
        if let Err(e) = crate::security::secret_store::persist_session_psk(&psk) {
            warn!("Desktop secret store persist failed: {}", e);
        }

        // Replace any prior cellular session cleanly.
        self.leave_cellular().await;

        let config = transport::CellularConfig {
            room_id: room_id.clone(),
            device_name: self.device_name.clone(),
            peer_id: format!("desk-{:08x}", self.device_id),
        };
        let cell = transport::CellularTransport::new(config, crypto, psk);
        cell.set_channel(channel); // stamp the right channel into relay wire frames

        // Await the first dial so auth/connect failures surface to the caller.
        cell.connect().await?;

        let audio_rx = cell
            .take_audio_receiver()
            .ok_or_else(|| "cellular receiver unavailable".to_string())?;

        // Reuse the shared decode → audio_cache → output pipeline.
        let handle = spawn_audio_rx_loop(
            Arc::clone(&self.audio),
            audio_rx,
            Arc::clone(&self.is_receiving),
            Arc::clone(&self.is_transmitting),
            Arc::clone(&self.audio_cache),
        );
        *self.cellular_rx_thread.lock().await = Some(handle);
        *self.cellular.lock().await = Some(cell);

        self.current_channel.store(channel, Ordering::Relaxed);
        *self.connection_status.write().await = ConnectionStatus::Connected;

        info!("Joined cellular room '{}' on channel {}", room_id, channel);
        Ok(room_id)
    }

    /// Leave the cellular relay session (idempotent).
    pub async fn leave_cellular(&self) {
        if let Some(cell) = self.cellular.lock().await.take() {
            cell.stop();
        }
        if let Some(handle) = self.cellular_rx_thread.lock().await.take() {
            handle.abort();
        }
        *self.connection_status.write().await = ConnectionStatus::Disconnected;
    }

    /// Clear keys, control plane, and persisted session material. In-app wipe
    /// hook; enterprise silent wipe still requires MDM/EMM.
    pub async fn wipe_session(&self) {
        self.leave_cellular().await;
        self.transport.lock().await.clear_session_psk();
        let _ = crate::security::secret_store::wipe_session_psk();
        info!("Session wiped (keys, control plane, secret store)");
    }

    /// Cellular session stats as JSON, or None if not joined.
    pub async fn cellular_status(&self) -> Option<String> {
        self.cellular.lock().await.as_ref().map(|c| c.stats_json())
    }

    /// True if a cellular relay session is currently active.
    pub async fn is_cellular_active(&self) -> bool {
        self.cellular.lock().await.is_some()
    }

    /// Start transmitting (PTT press)
    pub async fn start_transmit(&self) -> Result<(), AppError> {
        if self.is_transmitting.load(Ordering::Relaxed) {
            return Err(AppError::AlreadyTransmitting);
        }

        info!("Starting transmission");

        self.is_transmitting.store(true, Ordering::Relaxed);
        *self.connection_status.write().await = ConnectionStatus::Transmitting;

        // Start audio recording
        let audio = self.audio.lock().await;
        audio.start_recording()?;
        drop(audio);

        // Start TX thread
        self.start_tx_thread().await;

        Ok(())
    }

    /// Stop transmitting (PTT release)
    pub async fn stop_transmit(&self) -> Result<(), AppError> {
        if !self.is_transmitting.load(Ordering::Relaxed) {
            return Err(AppError::NotTransmitting);
        }

        info!("Stopping transmission");

        self.is_transmitting.store(false, Ordering::Relaxed);

        // Stop TX thread
        self.stop_tx_thread().await;

        // Stop audio recording
        let audio = self.audio.lock().await;
        audio.stop_recording()?;
        drop(audio);

        // Send roger beep if enabled (network + local)
        if self.roger_beep.load(Ordering::Relaxed) {
            // Send over network to peers
            self.send_roger_beep().await;

            // Play locally - use spawn_blocking since Stream is !Send
            let tone_player = Arc::clone(&self.tone_player);
            tokio::task::spawn_blocking(move || {
                if let Err(e) = tone_player.play_sync(ToneType::RogerBeep) {
                    warn!("Failed to play local roger beep: {}", e);
                }
            });
        }

        *self.connection_status.write().await = ConnectionStatus::Connected;

        Ok(())
    }

    /// Start TX thread (recording and encoding)
    async fn start_tx_thread(&self) {
        let audio = Arc::clone(&self.audio);
        let transport = Arc::clone(&self.transport);
        let is_transmitting = Arc::clone(&self.is_transmitting);
        let _channel = self.current_channel.load(Ordering::Relaxed);
        // Snapshot transports at PTT-press time and fan out to every live plane
        // (relay + UDP). XOR-routing here silenced mixed-protocol peers.
        let cellular = self.cellular.lock().await.clone();

        let handle = tokio::spawn(async move {
            let mut encoder = match OpusEncoder::new() {
                Ok(e) => e,
                Err(e) => {
                    error!("Failed to create encoder: {}", e);
                    // The caller set is_transmitting=true before spawning this task.
                    // Clear it on encoder-init failure, otherwise PTT stays stuck
                    // "on" (start_transmit refuses to re-arm) and the user can
                    // never transmit again until app restart.
                    is_transmitting.store(false, Ordering::Relaxed);
                    return;
                }
            };

            let mut buffer = vec![0i16; FRAME_SIZE];

            while is_transmitting.load(Ordering::Relaxed) {
                // Read audio samples
                let audio_lock = audio.lock().await;
                let samples_read = audio_lock.read_samples(&mut buffer);
                drop(audio_lock);

                if samples_read == FRAME_SIZE {
                    // Encode to Opus
                    match encoder.encode(&buffer) {
                        Ok(opus_data) => {
                            send_opus_fanout(&opus_data, &cellular, &transport).await;
                        }
                        Err(e) => {
                            error!("Encoding error: {}", e);
                        }
                    }
                } else {
                    // Not enough samples yet, wait briefly
                    tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
                }
            }

            info!("TX thread stopped");
        });

        *self.tx_thread.lock().await = Some(handle);
    }

    /// Stop TX thread
    async fn stop_tx_thread(&self) {
        if let Some(handle) = self.tx_thread.lock().await.take() {
            handle.abort();
        }
    }

    /// Start RX thread (receiving and decoding) for the UDP transport.
    async fn start_rx_thread(&self) -> Result<(), AppError> {
        // Get audio receiver from the UDP transport (yields decrypted Opus).
        let audio_rx = {
            let transport_lock = self.transport.lock().await;
            transport_lock
                .take_audio_receiver()
                .ok_or(AppError::NotConnected)?
        };

        let handle = spawn_audio_rx_loop(
            Arc::clone(&self.audio),
            audio_rx,
            Arc::clone(&self.is_receiving),
            Arc::clone(&self.is_transmitting),
            Arc::clone(&self.audio_cache),
        );

        *self.rx_thread.lock().await = Some(handle);

        Ok(())
    }

    /// Stop RX thread
    async fn stop_rx_thread(&self) {
        if let Some(handle) = self.rx_thread.lock().await.take() {
            handle.abort();
        }
    }

    /// Get nearby devices
    pub async fn get_nearby_devices(&self) -> Vec<PeerInfo> {
        let transport = self.transport.lock().await;
        transport.get_peers()
    }

    /// Get connection status
    pub async fn get_connection_status(&self) -> ConnectionStatus {
        *self.connection_status.read().await
    }

    /// Get current channel
    pub fn get_channel(&self) -> u8 {
        self.current_channel.load(Ordering::Relaxed)
    }

    /// True while the RX loop is decoding inbound audio.
    pub fn is_receiving(&self) -> bool {
        self.is_receiving.load(Ordering::Relaxed)
    }

    /// Set channel (clamped to valid range 1-16)
    pub async fn set_channel(&self, channel: u8) {
        let channel = channel.clamp(1, 16);
        self.current_channel.store(channel, Ordering::Relaxed);

        {
            let transport = self.transport.lock().await;
            transport.set_channel(channel);
        }
        // Keep the relay's wire-frame channel in sync so its payload stays
        // byte-compatible with the phone after a channel change.
        if let Some(cell) = self.cellular.lock().await.as_ref() {
            cell.set_channel(channel);
        }
    }

    /// Get audio devices
    pub async fn get_audio_devices(&self) -> (Vec<AudioDeviceInfo>, Vec<AudioDeviceInfo>) {
        let audio = self.audio.lock().await;
        let inputs = audio.get_input_devices();
        let outputs = audio.get_output_devices();
        (inputs, outputs)
    }

    /// Set input device
    pub async fn set_input_device(&self, device_name: &str) -> Result<(), AppError> {
        let mut audio = self.audio.lock().await;
        audio.set_input_device(device_name)?;
        Ok(())
    }

    /// Set output device
    pub async fn set_output_device(&self, device_name: &str) -> Result<(), AppError> {
        let mut audio = self.audio.lock().await;
        audio.set_output_device(device_name)?;
        Ok(())
    }

    /// Get volume
    pub async fn get_volume(&self) -> (f32, f32) {
        let audio = self.audio.lock().await;
        (audio.get_input_volume(), audio.get_output_volume())
    }

    /// Set volume
    pub async fn set_volume(&self, input: f32, output: f32) {
        let audio = self.audio.lock().await;
        audio.set_input_volume(input);
        audio.set_output_volume(output);
    }

    /// Set roger beep
    pub fn set_roger_beep(&self, enabled: bool) {
        self.roger_beep.store(enabled, Ordering::Relaxed);
    }

    /// Set VOX enabled
    pub fn set_vox_enabled(&self, enabled: bool) {
        self.vox_enabled.store(enabled, Ordering::Relaxed);
    }

    /// Set VOX threshold
    pub async fn set_vox_threshold(&self, threshold: f32) {
        *self.vox_threshold.write().await = threshold;
    }

    /// Send roger beep tone
    async fn send_roger_beep(&self) {
        // Generate a classic two-tone beep (800Hz + 1000Hz, 100ms total)
        let mut encoder = match OpusEncoder::new() {
            Ok(e) => e,
            Err(e) => {
                warn!("Failed to create encoder for roger beep: {}", e);
                return;
            }
        };

        // Route to whichever transport is active.
        let cellular = self.cellular.lock().await.clone();

        // Generate 100ms of dual-tone beep (about 5 frames at 20ms each)
        let frames_to_send = 5;
        let mut samples = vec![0i16; FRAME_SIZE];

        for frame_idx in 0..frames_to_send {
            // Generate dual-tone samples
            for (i, sample) in samples.iter_mut().enumerate() {
                let t = (frame_idx * FRAME_SIZE + i) as f32 / SAMPLE_RATE as f32;
                // 800Hz + 1000Hz dual tone with envelope
                let envelope = if frame_idx < 2 {
                    (frame_idx as f32 * FRAME_SIZE as f32 + i as f32) / (2.0 * FRAME_SIZE as f32)
                } else if frame_idx >= 3 {
                    1.0 - ((frame_idx as f32 - 3.0) * FRAME_SIZE as f32 + i as f32)
                        / (2.0 * FRAME_SIZE as f32)
                } else {
                    1.0
                };
                let tone = (f32::sin(2.0 * std::f32::consts::PI * 800.0 * t) * 0.5
                    + f32::sin(2.0 * std::f32::consts::PI * 1000.0 * t) * 0.5)
                    * envelope
                    * 8000.0;
                *sample = tone as i16;
            }

            // Encode and fan out so LAN and relay peers both hear the roger.
            if let Ok(opus_data) = encoder.encode(&samples) {
                send_opus_fanout(&opus_data, &cellular, &self.transport).await;
            }

            // Small delay between frames
            tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
        }

        info!("Roger beep sent");
    }

    /// Get device info
    pub fn get_device_info(&self) -> (u32, String) {
        (self.device_id, self.device_name.clone())
    }

    /// Get tone player
    pub fn get_tone_player(&self) -> Arc<TonePlayer> {
        Arc::clone(&self.tone_player)
    }

    /// Get transport configuration
    pub async fn get_transport_config(&self) -> TransportConfig {
        let transport = self.transport.lock().await;
        transport.get_config()
    }

    /// Update transport configuration
    pub async fn set_transport_config(&self, config: TransportConfig) {
        let transport = self.transport.lock().await;
        transport.update_config(config);
    }

    /// Get current bound port
    pub async fn get_port(&self) -> u16 {
        let transport = self.transport.lock().await;
        transport.get_port()
    }

    /// Check if encryption is active
    pub async fn is_encrypted(&self) -> bool {
        let transport = self.transport.lock().await;
        transport.is_encrypted()
    }

    /// Get our public key (hex encoded)
    pub async fn get_public_key(&self) -> Option<String> {
        let transport = self.transport.lock().await;
        transport.get_public_key()
    }

    /// UDP audio ingress queue health for diagnostics.
    pub async fn udp_queue_metrics(&self) -> transport::QueueMetricsSnapshot {
        let transport = self.transport.lock().await;
        transport.queue_metrics()
    }

    /// Get connection quality per peer
    pub async fn get_connection_quality(&self) -> Vec<(String, String, Option<u32>, String)> {
        let transport = self.transport.lock().await;
        transport.get_connection_quality()
    }
}

/// Shared RX pipeline: decode Opus → shared `sassytalkie-core` audio_cache
/// (Live / Queue / Mix) → audio output. Driven by either the UDP transport or
/// the cellular transport — both hand over a receiver of DECRYPTED Opus frames,
/// so the decode + playback path is identical and lives here once.
fn spawn_audio_rx_loop(
    audio: Arc<Mutex<AudioEngine>>,
    mut audio_rx: transport::RealtimeReceiver<transport::AudioFrame>,
    is_receiving: Arc<AtomicBool>,
    is_transmitting: Arc<AtomicBool>,
    cache: Arc<Mutex<sassytalkie_core::audio_cache::AudioCache>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut peers = RxPeerPool::new(MAX_RX_DECODERS, RX_DECODER_IDLE_TTL);

        // Start audio playback
        {
            let audio_lock = audio.lock().await;
            if let Err(e) = audio_lock.start_playing() {
                error!("Failed to start playback: {}", e);
                return;
            }
        }

        // The cache is shared across ALL RX loops (UDP + cellular) — its
        // (sender_id, timestamp) dedup only catches a dual-sent frame if both
        // copies land in the same instance. Keyed by sender_id, so it also
        // orders/queues multiple speakers correctly once each gets its own
        // decoder + real timestamp below.
        const FRAME_MS: u64 = 20;

        while let Some(frame) = audio_rx.recv().await {
            // Don't play audio while transmitting
            if is_transmitting.load(Ordering::Relaxed) {
                continue;
            }

            is_receiving.store(true, Ordering::Relaxed);

            let peer = peers.get_or_insert(&frame.sender, Instant::now());

            // Decode Opus to PCM with THIS sender's decoder; PLC on error.
            let pcm = match peer.decoder.decode(&frame.opus) {
                Ok(p) => p,
                Err(e) => {
                    error!("Decoding error from {}: {}", frame.sender, e);
                    peer.decoder.decode_plc().unwrap_or_default()
                }
            };
            if pcm.is_empty() {
                is_receiving.store(false, Ordering::Relaxed);
                continue;
            }

            // Real wire timestamp when present; else a per-sender synthetic clock
            // so the cache can still order this sender's frames by delta.
            let ts = if frame.timestamp != 0 {
                frame.timestamp
            } else {
                peer.synth_ts = peer.synth_ts.saturating_add(FRAME_MS);
                peer.synth_ts
            };

            // Live mode → Some(pcm) for immediate playback. Queue/Mix mode →
            // None; frames drain via the loop below on tick(). Hold the shared
            // cache lock across ingest+tick+drain so two RX loops can't
            // interleave mid-drain.
            {
                let mut cache = cache.lock().await;
                if let Some(samples) = cache.ingest_frame(&frame.sender, ts, pcm) {
                    let audio_lock = audio.lock().await;
                    audio_lock.write_samples(&samples);
                    drop(audio_lock);
                }

                cache.tick();
                while let Some((_sender, samples)) = cache.next_playback_frame() {
                    let audio_lock = audio.lock().await;
                    audio_lock.write_samples(&samples);
                    drop(audio_lock);
                }
            }

            is_receiving.store(false, Ordering::Relaxed);
        }

        // Stop audio playback
        {
            let audio_lock = audio.lock().await;
            let _ = audio_lock.stop_playing();
        }

        info!("RX thread stopped");
    })
}

const MAX_RX_DECODERS: usize = 32;
const RX_DECODER_IDLE_TTL: Duration = Duration::from_secs(30);

struct RxPeer {
    decoder: OpusDecoder,
    synth_ts: u64,
    last_used: Instant,
}

/// Per-sender Opus state with hard cardinality and idle-time bounds. On a full
/// pool the least-recently-used sender is evicted before a new decoder is made.
struct RxPeerPool {
    peers: HashMap<String, RxPeer>,
    max_peers: usize,
    idle_ttl: Duration,
}

impl RxPeerPool {
    fn new(max_peers: usize, idle_ttl: Duration) -> Self {
        assert!(max_peers > 0, "decoder pool must allow at least one sender");
        Self {
            peers: HashMap::new(),
            max_peers,
            idle_ttl,
        }
    }

    fn get_or_insert(&mut self, sender: &str, now: Instant) -> &mut RxPeer {
        self.peers.retain(|_, peer| {
            now.checked_duration_since(peer.last_used)
                .unwrap_or_default()
                <= self.idle_ttl
        });

        if !self.peers.contains_key(sender) && self.peers.len() >= self.max_peers {
            if let Some(oldest) = self
                .peers
                .iter()
                .min_by_key(|(_, peer)| peer.last_used)
                .map(|(id, _)| id.clone())
            {
                self.peers.remove(&oldest);
            }
        }

        let peer = self
            .peers
            .entry(sender.to_string())
            .or_insert_with(|| RxPeer {
                decoder: OpusDecoder::new().expect("create Opus decoder"),
                synth_ts: 0,
                last_used: now,
            });
        peer.last_used = now;
        peer
    }
}

#[cfg(test)]
mod rx_peer_pool_tests {
    use super::*;

    #[test]
    fn decoder_pool_never_exceeds_sender_cap() {
        let now = Instant::now();
        let mut pool = RxPeerPool::new(2, Duration::from_secs(60));
        pool.get_or_insert("a", now);
        pool.get_or_insert("b", now + Duration::from_millis(1));
        pool.get_or_insert("c", now + Duration::from_millis(2));

        assert_eq!(pool.peers.len(), 2);
        assert!(!pool.peers.contains_key("a"));
        assert!(pool.peers.contains_key("b"));
        assert!(pool.peers.contains_key("c"));
    }

    #[test]
    fn decoder_pool_expires_idle_senders() {
        let now = Instant::now();
        let mut pool = RxPeerPool::new(4, Duration::from_secs(5));
        pool.get_or_insert("stale", now);
        pool.get_or_insert("fresh", now + Duration::from_secs(6));

        assert_eq!(pool.peers.len(), 1);
        assert!(!pool.peers.contains_key("stale"));
        assert!(pool.peers.contains_key("fresh"));
    }
}
