// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-RDFGVZTYFWRR
//
//  SassyTalkie-Bridging-Header.h
//  SassyTalkie iOS
//
//  Copyright © 2025 Sassy Consulting LLC. All rights reserved.
//

#ifndef SassyTalkie_Bridging_Header_h
#define SassyTalkie_Bridging_Header_h

#import <Foundation/Foundation.h>

// Rust library FFI functions

/// Initialize the SassyTalkie library
bool sassytalkie_init(void);

/// Shutdown the library
void sassytalkie_shutdown(void);

/// Get version string (must free with sassytalkie_free_string)
const char* _Nonnull sassytalkie_get_version(void);

/// Free string allocated by library
void sassytalkie_free_string(char* _Nullable s);

/// Set current channel (1-99)
bool sassytalkie_set_channel(uint8_t channel);

/// Get current channel
uint8_t sassytalkie_get_channel(void);

/// Press PTT button (start transmission)
bool sassytalkie_ptt_press(void);

/// Release PTT button (stop transmission)
bool sassytalkie_ptt_release(void);

/// Connect to peer device
bool sassytalkie_connect(uint32_t device_id);

/// Disconnect from peer
bool sassytalkie_disconnect(void);

/// Start listening for incoming audio
bool sassytalkie_start_listening(void);

/// Get current state
/// 0=Idle, 1=Connecting, 2=Connected, 3=Transmitting, 4=Receiving, 5=Error
uint8_t sassytalkie_get_state(void);

/// Process audio input from AVAudioEngine
/// audio_data: PCM samples (16-bit signed)
/// sample_count: Number of samples
bool sassytalkie_process_audio_input(const int16_t* _Nonnull audio_data, size_t sample_count);

/// Get audio output for AVAudioEngine
/// buffer: Output buffer for PCM samples
/// buffer_size: Maximum samples to write
/// Returns: Number of samples written
size_t sassytalkie_get_audio_output(int16_t* _Nonnull buffer, size_t buffer_size);

// ── Bluetooth (CoreBluetooth bridge — Swift discovers, Rust tracks state) ──

/// BLE service UUID string (must free with sassytalkie_free_string)
const char* _Nonnull sassytalkie_bt_service_uuid(void);

/// Register a peer discovered by the Swift central
bool sassytalkie_bt_device_found(const char* _Nullable id, const char* _Nullable name, int32_t rssi);

/// Remove a peer reported lost / disconnected
bool sassytalkie_bt_device_lost(const char* _Nullable id);

/// Number of currently-discovered peers
size_t sassytalkie_bt_peer_count(void);

/// JSON array of discovered peers (must free with sassytalkie_free_string)
const char* _Nonnull sassytalkie_bt_get_peers_json(void);

/// Mark a peer as the actively-connected device
bool sassytalkie_bt_set_connected(const char* _Nullable id);

/// Clear the active BLE connection
void sassytalkie_bt_clear_connected(void);

// ── Crypto / key agreement (shared core — parity with Android) ──

/// Install a pre-shared key (QR session key) from base64 (32 bytes decoded).
/// After this, TX is AES-256-GCM encrypted and RX is decrypted + replay-checked.
bool sassytalkie_set_psk(const char* _Nullable key_b64);

/// Import a scanned QR session (the host's QR JSON). Switches to the QR's channel
/// and installs its key, reusing the shared core validation (same accept/reject
/// as Android). Returns the channel (1-8) on success, or 0 on a malformed/expired
/// QR. The cross-platform pairing entry point.
uint8_t sassytalkie_import_session_qr(const char* _Nullable qr_json);

/// Host a channel: mint a fresh session QR (the JSON another device scans) and
/// install its key locally so the host can TX/RX too. Returns the QR JSON to
/// render (must free with sassytalkie_free_string), or NULL on failure.
/// group_name may be NULL/empty; duration clamps to 1..=72 hours.
char* _Nullable sassytalkie_generate_session_qr(uint8_t channel, uint32_t duration_hours, const char* _Nullable group_name);

/// Decrypt a session-invite share blob (the bytes from GET /share/<id>) with the
/// url-safe-base64 key from the invite link's #fragment, via the shared core.
/// Returns the decrypted session-QR JSON (must free with sassytalkie_free_string)
/// to pass to sassytalkie_import_session_qr, or NULL on bad input/wrong key. The
/// iOS half of the https://relay.sassyconsultingllc.com/v/<id>#<key> import.
char* _Nullable sassytalkie_decrypt_share_blob(const uint8_t* _Nullable blob_ptr, size_t blob_len, const char* _Nullable key_b64url);

/// Begin a classical X25519 key exchange. Returns our base64 public key (must
/// free with sassytalkie_free_string). Complete with the function below.
char* _Nullable sassytalkie_key_exchange_init(void);

/// Complete the classical key exchange with the peer's base64 public key,
/// installing the AEAD session. Returns true on success.
bool sassytalkie_key_exchange_complete(const char* _Nullable remote_b64);

/// This build's capability bitmap (hybrid-PQC support) — same value Android
/// advertises in its heartbeat caps byte.
uint8_t sassytalkie_local_capabilities(void);

/// Begin a hybrid PQC handshake. Returns base64 initiator message (must free with
/// sassytalkie_free_string), or NULL if no PSK is installed / not initialized.
char* _Nullable sassytalkie_hybrid_handshake_init(void);

/// Responder: given the peer's base64 initiator message, install the session and
/// return the base64 responder message (must free with sassytalkie_free_string),
/// or NULL on failure.
/// Responder: stage the proposed session; install only after confirm.
char* _Nullable sassytalkie_hybrid_handshake_respond(const char* _Nullable init_b64);

/// Initiator: complete with the peer's base64 responder message, installing the
/// post-quantum session. Returns true on success.
bool sassytalkie_hybrid_handshake_complete(const char* _Nullable resp_b64);

/// Responder: install the staged PQ session after authenticated confirm.
bool sassytalkie_hybrid_handshake_confirm(void);

/// Wipe keys, control plane, and staged hybrid. In-app hook; MDM/EMM triggers remote logout.
void sassytalkie_wipe_session(void);

/// Technical audit JSON (free with sassytalkie_free_string). Not a legal chain of custody.
char* _Nullable sassytalkie_export_audit(void);

/// Optional MDM enrollment token. Empty/NULL clears. Room id is not authorization.
bool sassytalkie_set_enrollment_token(const char* _Nullable token);

// ── Relay (Cloudflare WebSocket) ──
// The WebSocket is owned by Swift (URLSessionWebSocketTask); these bridge the
// Rust crypto/queue. Binary frames cross as (ptr, len); free returned buffers
// with sassytalkie_free_bytes.

/// Relay room id (= QR session_id), or NULL if not paired (free with
/// sassytalkie_free_string).
char* _Nullable sassytalkie_relay_room_id(void);

/// Mark the relay connected (true) / disconnected (false).
void sassytalkie_relay_set_active(bool active);

/// Drain one sealed audio frame to send over the WS. Returns a buffer and writes
/// its length to out_len (free with sassytalkie_free_bytes), or NULL if empty.
uint8_t* _Nullable sassytalkie_relay_poll_outbound(size_t* _Nonnull out_len);

/// Build the next heartbeat frame to send over the WS (free with sassytalkie_free_bytes).
uint8_t* _Nullable sassytalkie_relay_heartbeat_frame(size_t* _Nonnull out_len);

/// Process a binary frame received from the relay WS. Returns true if it was
/// playable audio for our channel.
bool sassytalkie_relay_on_message(const uint8_t* _Nullable ptr, size_t len);

/// Free a buffer returned by sassytalkie_relay_poll_outbound / _heartbeat_frame.
void sassytalkie_free_bytes(uint8_t* _Nullable ptr, size_t len);

/// True once a QR/PSK session is installed (mandatory encryption).
bool sassytalkie_is_paired(void);

/// Floor occupancy (1.5 s stale / 300 ms drain). Never the 400 ms UI LED.
bool sassytalkie_floor_held(void);

/// UI "peer speaking" LED (400 ms).
bool sassytalkie_peer_speaking(void);

/// One-shot PTT rejection reason (must free with sassytalkie_free_string), or NULL.
char* _Nullable sassytalkie_take_ptt_reject(void);

/// Seal session-QR JSON for POST /share. Returns JSON
/// {"blob_b64":"...","key_b64url":"..."} (free with sassytalkie_free_string), or NULL.
char* _Nullable sassytalkie_encrypt_share_blob(const char* _Nullable json);

/// JSON array of SPKI SHA-256 pins for the relay host (free with sassytalkie_free_string).
char* _Nullable sassytalkie_tls_pins_json(void);

/// Production default: pin when the core pin-set has backups.
bool sassytalkie_tls_pinning_enabled(void);

#endif /* SassyTalkie_Bridging_Header_h */
