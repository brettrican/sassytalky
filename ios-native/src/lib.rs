// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-UMXPLHPKQURS
// SassyTalkie iOS Core Library
// Copyright 2025 Sassy Consulting LLC. All rights reserved.

//! iOS Core Library for SassyTalkie PTT Walkie-Talkie
//! 
//! This library provides the core Rust functionality for iOS,
//! with FFI bindings for Swift to call into.

pub mod audio;
pub mod bluetooth;
pub mod codec;
pub mod state;
pub mod transport;
pub mod control;
pub mod ffi;

pub use audio::{AudioEngine, AudioFrame};
pub use codec::{OpusEncoder, OpusDecoder};
pub use state::{StateMachine, AppState};

// Shared cross-platform crypto/session/PQC from the core crate — the SAME engine
// Android (android-native re-exports these) and desktop use. iOS previously had
// NO crypto; these bring it to parity. Re-exported as crate::{crypto, pqc,
// session} so the FFI/state code reads identically to the other platforms.
pub use sassytalkie_core::crypto;
pub use sassytalkie_core::pqc;
pub use sassytalkie_core::session;
// `share` — decrypt `/v/<id>#<key>` invite blobs through the same audited
// AES-GCM path Android and desktop use (parity for link-import).
pub use sassytalkie_core::share;

use std::os::raw::c_char;
use std::ffi::{CStr, CString};
use std::sync::{Mutex, OnceLock};
use log::info;
use base64::Engine as _;

/// Base64-decode a C string argument to bytes, or None on null/invalid input.
unsafe fn decode_b64_arg(p: *const c_char) -> Option<Vec<u8>> {
    if p.is_null() { return None; }
    let s = CStr::from_ptr(p).to_str().ok()?;
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

/// Base64-encode bytes into a freshly-allocated C string (free with
/// sassytalkie_free_string), mirroring the JNI handshake return values.
fn encode_b64_cstring(bytes: &[u8]) -> *mut c_char {
    let s = base64::engine::general_purpose::STANDARD.encode(bytes);
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Global app state. `OnceLock<Mutex<Option<_>>>` (not `static mut`) so the
/// Swift audio/render thread and the UI/timer thread share it without UB; the
/// inner `Option` lets `sassytalkie_shutdown` clear it. Mirrors BT_MANAGER.
static APP_STATE: OnceLock<Mutex<Option<StateMachine>>> = OnceLock::new();

fn app_state() -> &'static Mutex<Option<StateMachine>> {
    APP_STATE.get_or_init(|| Mutex::new(None))
}

/// Initialize the library
/// 
/// # Safety
/// This function must be called before any other library functions
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_init() -> bool {
    // Initialize logger
    env_logger::init();
    info!("SassyTalkie iOS v{} initializing...", VERSION);
    
    // Create state machine
    match StateMachine::new() {
        Ok(state) => {
            if let Ok(mut g) = app_state().lock() {
                *g = Some(state);
            }
            info!("SassyTalkie initialized successfully");
            true
        }
        Err(e) => {
            eprintln!("Failed to initialize SassyTalkie: {}", e);
            false
        }
    }
}

/// Shutdown the library
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_shutdown() {
    info!("SassyTalkie shutting down...");
    if let Ok(mut g) = app_state().lock() {
        if let Some(mut s) = g.take() {
            let _ = s.shutdown();
        }
    }
}

/// Get version string
/// 
/// # Safety
/// Caller must free the returned string with `sassytalkie_free_string`
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_get_version() -> *const c_char {
    CString::new(VERSION).unwrap().into_raw()
}

/// Free a string allocated by the library
/// 
/// # Safety
/// Pointer must have been returned by a library function
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_free_string(s: *mut c_char) {
    if !s.is_null() {
        let _ = CString::from_raw(s);
    }
}

// ── Crypto / key agreement FFI (mirrors android-native's JNI crypto seam) ──

/// Install a pre-shared key (the QR session key) from base64. Returns true on
/// success. After this, TX frames are AES-256-GCM encrypted and RX frames are
/// decrypted (+ replay-checked).
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_set_psk(key_b64: *const c_char) -> bool {
    let bytes = match decode_b64_arg(key_b64) { Some(b) => b, None => return false };
    if bytes.len() != 32 { return false; }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    if let Ok(g) = app_state().lock() {
        if let Some(s) = g.as_ref() { s.set_psk(&key); return true; }
    }
    false
}

/// Import a scanned QR session — the JSON an Android/desktop host generates.
/// Switches to the QR's channel and installs its key, reusing the SHARED core
/// validation so iOS accepts/rejects exactly the QRs Android does. Returns the
/// channel (1-8) on success, or 0 on a malformed / expired QR. This is the
/// cross-platform pairing entry point: scan a host's QR → encrypted audio flows.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_import_session_qr(qr_json: *const c_char) -> u8 {
    let json = match ffi::helpers::c_string_to_rust(qr_json) {
        Some(s) if !s.is_empty() => s,
        _ => return 0,
    };
    if let Ok(g) = app_state().lock() {
        if let Some(s) = g.as_ref() { return s.import_session_qr(&json).unwrap_or(0); }
    }
    0
}

/// Decrypt a session-invite share blob (the bytes from `GET /share/<id>`) using
/// the url-safe-base64 key from the invite link's `#fragment`, via the SHARED
/// core. Returns the decrypted session-QR JSON (free with
/// `sassytalkie_free_string`) to hand straight to `sassytalkie_import_session_qr`,
/// or null on bad input / wrong key / tampered blob.
///
/// `blob_ptr`/`blob_len` are the raw relay bytes (`IV‖ciphertext+tag`). The key
/// never reaches the relay, so a KV dump is useless. This is the iOS half of the
/// `/v/<id>#<key>` invite import — the exact `core::share` path Android (Kotlin)
/// and desktop (Tauri) use.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_decrypt_share_blob(
    blob_ptr: *const u8,
    blob_len: usize,
    key_b64url: *const c_char,
) -> *mut c_char {
    if blob_ptr.is_null() || blob_len == 0 {
        return std::ptr::null_mut();
    }
    let key = match ffi::helpers::c_string_to_rust(key_b64url) {
        Some(s) if !s.is_empty() => s,
        _ => return std::ptr::null_mut(),
    };
    let blob = std::slice::from_raw_parts(blob_ptr, blob_len);
    match share::decrypt_share_blob(blob, &key) {
        Ok(json) => match CString::new(json) {
            Ok(c) => c.into_raw(),
            Err(_) => std::ptr::null_mut(),
        },
        Err(_) => std::ptr::null_mut(),
    }
}

/// Host a channel: mint a fresh session QR (the JSON another device scans) and
/// install its key locally so the host can TX/RX on this channel too. Returns the
/// QR JSON to render (free with sassytalkie_free_string), or null on failure.
/// `group_name` may be null/empty ("Channel N"); duration clamps to 1..=72 h.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_generate_session_qr(
    channel: u8,
    duration_hours: u32,
    group_name: *const c_char,
) -> *mut c_char {
    let group = ffi::helpers::c_string_to_rust(group_name).unwrap_or_default();
    let json = {
        let g = match app_state().lock() { Ok(g) => g, Err(_) => return std::ptr::null_mut() };
        match g.as_ref().and_then(|s| s.generate_session_qr(channel, duration_hours, &group)) {
            Some(j) => j,
            None => return std::ptr::null_mut(),
        }
    };
    match CString::new(json) {
        Ok(c) => c.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Begin a classical X25519 key exchange. Returns our base64 public key (free
/// with sassytalkie_free_string), or null. Complete with
/// sassytalkie_key_exchange_complete.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_key_exchange_init() -> *mut c_char {
    let pubkey = {
        let g = match app_state().lock() { Ok(g) => g, Err(_) => return std::ptr::null_mut() };
        match g.as_ref() {
            Some(s) => s.key_exchange_init(),
            None => return std::ptr::null_mut(),
        }
    };
    encode_b64_cstring(&pubkey)
}

/// Complete the classical key exchange with the peer's base64 public key,
/// installing the AEAD session. Returns true on success.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_key_exchange_complete(remote_b64: *const c_char) -> bool {
    let bytes = match decode_b64_arg(remote_b64) { Some(b) => b, None => return false };
    if bytes.len() != 32 { return false; }
    let mut remote = [0u8; 32];
    remote.copy_from_slice(&bytes);
    if let Ok(g) = app_state().lock() {
        if let Some(s) = g.as_ref() { return s.key_exchange_complete(&remote); }
    }
    false
}

/// Wipe session keys, control plane, and staged hybrid. In-app hook; MDM/EMM
/// must trigger this for remote logout.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_wipe_session() {
    if let Ok(g) = app_state().lock() {
        if let Some(s) = g.as_ref() { s.wipe_session(); }
    }
}

/// Technical audit JSON (free with sassytalkie_free_string). Not a legal chain of custody.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_export_audit() -> *mut c_char {
    if let Ok(g) = app_state().lock() {
        if let Some(s) = g.as_ref() {
            return match CString::new(s.export_audit()) {
                Ok(c) => c.into_raw(),
                Err(_) => std::ptr::null_mut(),
            };
        }
    }
    std::ptr::null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn sassytalkie_set_enrollment_token(token: *const c_char) -> bool {
    let value = ffi::helpers::c_string_to_rust(token);
    if let Ok(g) = app_state().lock() {
        if let Some(s) = g.as_ref() {
            s.set_enrollment_token(value);
            return true;
        }
    }
    false
}

#[no_mangle]
pub unsafe extern "C" fn sassytalkie_hybrid_handshake_confirm() -> bool {
    if let Ok(g) = app_state().lock() {
        if let Some(s) = g.as_ref() { return s.hybrid_confirm(); }
    }
    false
}

/// This build's capability bitmap (hybrid-PQC support) — same value Android
/// advertises in its heartbeat caps byte.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_local_capabilities() -> u8 {
    if let Ok(g) = app_state().lock() {
        if let Some(s) = g.as_ref() { return s.local_capabilities(); }
    }
    0
}

/// Initiator: begin a path-(a) PSK-authenticated hybrid handshake. Returns the
/// base64 initiator message (free with sassytalkie_free_string), or null if no
/// PSK is installed / not initialized.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_hybrid_handshake_init() -> *mut c_char {
    let msg = {
        let g = match app_state().lock() { Ok(g) => g, Err(_) => return std::ptr::null_mut() };
        match g.as_ref().and_then(|s| s.hybrid_init()) {
            Some(m) => m,
            None => return std::ptr::null_mut(),
        }
    };
    encode_b64_cstring(&msg)
}

/// Responder: given the peer's base64 initiator message, install the session and
/// return the base64 responder message (free with sassytalkie_free_string), or
/// null on failure.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_hybrid_handshake_respond(init_b64: *const c_char) -> *mut c_char {
    let init_bytes = match decode_b64_arg(init_b64) { Some(b) => b, None => return std::ptr::null_mut() };
    let resp = {
        let g = match app_state().lock() { Ok(g) => g, Err(_) => return std::ptr::null_mut() };
        match g.as_ref().and_then(|s| s.hybrid_respond(&init_bytes)) {
            Some(r) => r,
            None => return std::ptr::null_mut(),
        }
    };
    encode_b64_cstring(&resp)
}

/// Initiator: complete with the peer's base64 responder message, installing the
/// post-quantum session. Returns true on success. Must follow a
/// sassytalkie_hybrid_handshake_init on this device.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_hybrid_handshake_complete(resp_b64: *const c_char) -> bool {
    let resp_bytes = match decode_b64_arg(resp_b64) { Some(b) => b, None => return false };
    if let Ok(g) = app_state().lock() {
        if let Some(s) = g.as_ref() { return s.hybrid_complete(&resp_bytes); }
    }
    false
}

// ── Relay (Cloudflare WebSocket) FFI ────────────────────────────────────────
// The WebSocket itself is owned by Swift (URLSessionWebSocketTask); these expose
// the Rust crypto/queue bridge. Binary frames cross the C ABI as (ptr, len); the
// caller MUST free a returned buffer with `sassytalkie_free_bytes`.

/// Move a Vec<u8> across the C ABI as an owned buffer. `into_boxed_slice` forces
/// capacity == length so `sassytalkie_free_bytes` can reconstruct it exactly.
fn bytes_into_raw(v: Vec<u8>, out_len: *mut usize) -> *mut u8 {
    let boxed = v.into_boxed_slice();
    let len = boxed.len();
    let ptr = Box::into_raw(boxed) as *mut u8;
    unsafe { *out_len = len; }
    ptr
}

/// Free a buffer returned by `sassytalkie_relay_poll_outbound` /
/// `sassytalkie_relay_heartbeat_frame`.
///
/// # Safety
/// `ptr`/`len` must be exactly a pair previously returned by those functions.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_free_bytes(ptr: *mut u8, len: usize) {
    if ptr.is_null() { return; }
    let slice = std::slice::from_raw_parts_mut(ptr, len);
    let _ = Box::from_raw(slice as *mut [u8]);
}

/// The relay room id (= QR session_id) for the active session, or null if not
/// paired. Free with `sassytalkie_free_string`.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_relay_room_id() -> *mut c_char {
    let id = {
        let g = match app_state().lock() { Ok(g) => g, Err(_) => return std::ptr::null_mut() };
        match g.as_ref().and_then(|s| s.relay_room_id()) {
            Some(r) => r,
            None => return std::ptr::null_mut(),
        }
    };
    CString::new(id).map(|c| c.into_raw()).unwrap_or(std::ptr::null_mut())
}

/// Mark the relay connected (true) / disconnected (false). While connected, TX
/// frames are teed into the relay outbound queue for Swift to forward.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_relay_set_active(active: bool) {
    if let Ok(g) = app_state().lock() {
        if let Some(s) = g.as_ref() { s.set_relay_active(active); }
    }
}

/// Drain one sealed audio frame to send over the WebSocket. Writes the length to
/// `out_len` and returns a buffer (free with `sassytalkie_free_bytes`), or null
/// when the queue is empty.
///
/// # Safety
/// `out_len` must be a valid pointer to a usize.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_relay_poll_outbound(out_len: *mut usize) -> *mut u8 {
    if out_len.is_null() { return std::ptr::null_mut(); }
    *out_len = 0;
    let frame = {
        let g = match app_state().lock() { Ok(g) => g, Err(_) => return std::ptr::null_mut() };
        match g.as_ref().and_then(|s| s.poll_relay_outbound()) {
            Some(f) => f,
            None => return std::ptr::null_mut(),
        }
    };
    bytes_into_raw(frame, out_len)
}

/// Build the next OP_HEARTBEAT frame to send over the WebSocket (~every 2 s).
/// Free with `sassytalkie_free_bytes`. Null only if not initialized.
///
/// # Safety
/// `out_len` must be a valid pointer to a usize.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_relay_heartbeat_frame(out_len: *mut usize) -> *mut u8 {
    if out_len.is_null() { return std::ptr::null_mut(); }
    *out_len = 0;
    let frame = {
        let g = match app_state().lock() { Ok(g) => g, Err(_) => return std::ptr::null_mut() };
        match g.as_ref() {
            Some(s) => s.relay_heartbeat_frame(),
            None => return std::ptr::null_mut(),
        }
    };
    bytes_into_raw(frame, out_len)
}

/// Process a binary frame received from the relay WebSocket: decrypt + unpack +
/// decode + play. Returns true if it was a playable audio frame for our channel
/// (false = control/heartbeat from a peer, wrong channel, or not for us).
///
/// # Safety
/// `ptr` must point to `len` valid bytes.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_relay_on_message(ptr: *const u8, len: usize) -> bool {
    if ptr.is_null() || len == 0 { return false; }
    let bytes = std::slice::from_raw_parts(ptr, len);
    if let Ok(g) = app_state().lock() {
        if let Some(s) = g.as_ref() { return s.process_relay_frame(bytes); }
    }
    false
}

/// Set current channel
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_set_channel(channel: u8) -> bool {
    if let Ok(mut g) = app_state().lock() {
        if let Some(s) = g.as_mut() {
            s.set_channel(channel);
            return true;
        }
    }
    false
}

/// Get current channel
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_get_channel() -> u8 {
    if let Ok(g) = app_state().lock() {
        if let Some(s) = g.as_ref() {
            return s.get_channel();
        }
    }
    1 // Default channel
}

/// Start PTT transmission
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_ptt_press() -> bool {
    if let Ok(mut g) = app_state().lock() {
        if let Some(s) = g.as_mut() {
            return s.on_ptt_press().is_ok();
        }
    }
    false
}

/// Stop PTT transmission
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_ptt_release() -> bool {
    if let Ok(mut g) = app_state().lock() {
        if let Some(s) = g.as_mut() {
            return s.on_ptt_release().is_ok();
        }
    }
    false
}

/// Connect to a peer device
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_connect(device_id: u32) -> bool {
    if let Ok(mut g) = app_state().lock() {
        if let Some(s) = g.as_mut() {
            return s.connect_to_device(device_id).is_ok();
        }
    }
    false
}

/// Disconnect from peer
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_disconnect() -> bool {
    if let Ok(mut g) = app_state().lock() {
        if let Some(s) = g.as_mut() {
            return s.disconnect().is_ok();
        }
    }
    false
}

/// Start listening for incoming connections
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_start_listening() -> bool {
    if let Ok(mut g) = app_state().lock() {
        if let Some(s) = g.as_mut() {
            return s.start_listening().is_ok();
        }
    }
    false
}

/// True once a QR/PSK session is installed (mandatory encryption).
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_is_paired() -> bool {
    if let Ok(g) = app_state().lock() {
        if let Some(s) = g.as_ref() {
            return s.is_paired();
        }
    }
    false
}

/// Floor occupancy (1.5 s stale hold / 300 ms drain) — NOT the 400 ms UI LED.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_floor_held() -> bool {
    if let Ok(g) = app_state().lock() {
        if let Some(s) = g.as_ref() {
            return s.floor().is_held(crate::control::now_ms());
        }
    }
    false
}

/// UI "peer speaking" LED (400 ms). Never use this as the TX lock.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_peer_speaking() -> bool {
    if let Ok(g) = app_state().lock() {
        if let Some(s) = g.as_ref() {
            return s.floor().peer_speaking(crate::control::now_ms());
        }
    }
    false
}

/// One-shot PTT rejection reason ("Channel busy", …). Free with
/// `sassytalkie_free_string`. NULL if the last press succeeded.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_take_ptt_reject() -> *mut c_char {
    if let Ok(g) = app_state().lock() {
        if let Some(s) = g.as_ref() {
            if let Some(reason) = s.floor().take_reject_reason() {
                return CString::new(reason).map(|c| c.into_raw()).unwrap_or(std::ptr::null_mut());
            }
        }
    }
    std::ptr::null_mut()
}

/// Seal session-QR JSON into a share blob. Returns JSON
/// `{"blob_b64":"<standard>","key_b64url":"<url-safe>"}` (free with
/// `sassytalkie_free_string`), or NULL. The key belongs in the invite fragment.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_encrypt_share_blob(json: *const c_char) -> *mut c_char {
    let json = match ffi::helpers::c_string_to_rust(json) {
        Some(s) if !s.is_empty() => s,
        _ => return std::ptr::null_mut(),
    };
    match share::encrypt_share_blob(&json) {
        Ok((blob, key_b64url)) => {
            let blob_b64 = base64::engine::general_purpose::STANDARD.encode(&blob);
            let out = serde_json::json!({
                "blob_b64": blob_b64,
                "key_b64url": key_b64url,
            });
            match CString::new(out.to_string()) {
                Ok(c) => c.into_raw(),
                Err(_) => std::ptr::null_mut(),
            }
        }
        Err(_) => std::ptr::null_mut(),
    }
}

/// JSON array of SPKI SHA-256 pins (base64) for `relay.sassyconsultingllc.com`.
/// Free with `sassytalkie_free_string`. Single source: `core::tls_pins`.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_tls_pins_json() -> *mut c_char {
    let json = serde_json::to_string(sassytalkie_core::tls_pins::SPKI_PINS_SHA256_B64)
        .unwrap_or_else(|_| "[]".into());
    CString::new(json).map(|c| c.into_raw()).unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn sassytalkie_tls_pinning_enabled() -> bool {
    sassytalkie_core::tls_pins::PINNING_DEFAULT && sassytalkie_core::tls_pins::pins_complete()
}

/// Get current state (0=Idle, 1=Connecting, 2=Connected, 3=Transmitting, 4=Receiving)
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_get_state() -> u8 {
    if let Ok(g) = app_state().lock() {
        if let Some(s) = g.as_ref() {
            return match s.current_state() {
                AppState::Idle => 0,
                AppState::Connecting => 1,
                AppState::Connected => 2,
                AppState::Transmitting => 3,
                AppState::Receiving => 4,
                AppState::Error => 5,
            };
        }
    }
    0
}

/// Process audio input from Swift (AVAudioEngine)
/// 
/// # Safety
/// `audio_data` must point to valid PCM samples
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_process_audio_input(
    audio_data: *const i16,
    sample_count: usize,
) -> bool {
    if audio_data.is_null() || sample_count == 0 {
        return false;
    }
    
    let samples = std::slice::from_raw_parts(audio_data, sample_count);
    
    if let Ok(mut g) = app_state().lock() {
        if let Some(s) = g.as_mut() {
            return s.process_audio_input(samples).is_ok();
        }
    }
    false
}

/// Get audio output for Swift (AVAudioEngine)
/// Returns number of samples written
/// 
/// # Safety
/// `buffer` must have space for at least `buffer_size` samples
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_get_audio_output(
    buffer: *mut i16,
    buffer_size: usize,
) -> usize {
    if buffer.is_null() || buffer_size == 0 {
        return 0;
    }
    
    let out_buffer = std::slice::from_raw_parts_mut(buffer, buffer_size);
    
    if let Ok(mut g) = app_state().lock() {
        if let Some(s) = g.as_mut() {
            return s.get_audio_output(out_buffer).unwrap_or(0);
        }
    }
    0
}

// ───────────────────────── Bluetooth peer-finding (CoreBluetooth bridge) ─────────────────────────
//
// On iOS, Bluetooth is the PEER-DISCOVERY plane only — audio rides the IP transport
// (iOS does not expose Bluetooth Classic / RFCOMM to third-party apps). The radio
// work (BLE advertise + scan) lives in Swift via CoreBluetooth (BluetoothManager.swift).
// Swift reports discovered / lost peers across this FFI; Rust keeps the canonical
// roster and hands it back to the UI as JSON.
//
// The advertised + scanned GATT service UUID MUST match the Android app so iOS↔Android
// peers discover each other.

use crate::bluetooth::{BluetoothManager, BluetoothDevice};

/// SassyTalkie BLE service UUID — identical to Android `BleSignalingService.SERVICE_UUID`.
pub const SASSYTALKIE_BLE_SERVICE_UUID: &str = "b1a2e5d4-d5ab-7890-bede-fa12345678f0";

/// Process-wide BLE peer roster. `OnceLock<Mutex<_>>` (not `static mut`) so the
/// Swift radio thread and the Rust UI-query thread share it without UB.
static BT_MANAGER: OnceLock<Mutex<BluetoothManager>> = OnceLock::new();

fn bt_manager() -> &'static Mutex<BluetoothManager> {
    BT_MANAGER.get_or_init(|| Mutex::new(BluetoothManager::new()))
}

/// Return the BLE service UUID that Swift should advertise + scan for.
///
/// # Safety
/// Caller must free the returned string with `sassytalkie_free_string`.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_bt_service_uuid() -> *const c_char {
    CString::new(SASSYTALKIE_BLE_SERVICE_UUID).unwrap().into_raw()
}

/// Register a peer discovered by the Swift CoreBluetooth central.
///
/// # Safety
/// `id` and `name` must be valid NUL-terminated UTF-8 C strings (or null).
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_bt_device_found(
    id: *const c_char,
    name: *const c_char,
    rssi: i32,
) -> bool {
    let id = match ffi::helpers::c_string_to_rust(id) {
        Some(s) if !s.is_empty() => s,
        _ => return false,
    };
    let name = ffi::helpers::c_string_to_rust(name).unwrap_or_else(|| "SassyTalkie".to_string());
    if let Ok(mut mgr) = bt_manager().lock() {
        mgr.add_device(BluetoothDevice { id, name, rssi });
        info!("BLE peer discovered (rssi={})", rssi);
        true
    } else {
        false
    }
}

/// Remove a peer the Swift central reported as lost / disconnected.
///
/// # Safety
/// `id` must be a valid NUL-terminated UTF-8 C string (or null).
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_bt_device_lost(id: *const c_char) -> bool {
    let id = match ffi::helpers::c_string_to_rust(id) {
        Some(s) => s,
        None => return false,
    };
    if let Ok(mut mgr) = bt_manager().lock() {
        mgr.remove_device(&id);
        true
    } else {
        false
    }
}

/// Number of currently-discovered BLE peers.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_bt_peer_count() -> usize {
    bt_manager().lock().map(|m| m.devices().len()).unwrap_or(0)
}

/// JSON array of discovered peers: `[{"id":..,"name":..,"rssi":..}, ...]`.
///
/// # Safety
/// Caller must free the returned string with `sassytalkie_free_string`.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_bt_get_peers_json() -> *const c_char {
    let json = bt_manager()
        .lock()
        .ok()
        .map(|m| serde_json::to_string(&m.devices()).unwrap_or_else(|_| "[]".to_string()))
        .unwrap_or_else(|| "[]".to_string());
    CString::new(json).unwrap_or_default().into_raw()
}

/// Mark a peer as the actively-connected device.
///
/// # Safety
/// `id` must be a valid NUL-terminated UTF-8 C string (or null).
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_bt_set_connected(id: *const c_char) -> bool {
    let id = match ffi::helpers::c_string_to_rust(id) {
        Some(s) => s,
        None => return false,
    };
    if let Ok(mut mgr) = bt_manager().lock() {
        mgr.set_connected(id);
        true
    } else {
        false
    }
}

/// Clear the active BLE connection.
#[no_mangle]
pub unsafe extern "C" fn sassytalkie_bt_clear_connected() {
    if let Ok(mut mgr) = bt_manager().lock() {
        mgr.clear_connected();
    }
}
