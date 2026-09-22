// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-WGFEYW3GJF44
/// Tauri Commands - Frontend API
///
/// These commands are called from the React frontend via invoke()
use crate::{AppState, AudioDeviceInfo, ConnectionStatus, PeerInfo};
use std::sync::Arc;
use tauri::State;

/// Start discovery and listening for peers
#[tauri::command]
pub async fn start_discovery(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.start_discovery().await.map_err(|e| e.to_string())
}

/// Stop discovery
#[tauri::command]
pub async fn stop_discovery(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.stop_discovery().await.map_err(|e| e.to_string())
}

/// Get list of nearby devices
#[tauri::command]
pub async fn get_nearby_devices(state: State<'_, Arc<AppState>>) -> Result<Vec<PeerInfo>, String> {
    Ok(state.get_nearby_devices().await)
}

/// Connect to a peer (not used in multicast mode, but kept for API compatibility)
#[tauri::command]
pub async fn connect_to_peer(
    _state: State<'_, Arc<AppState>>,
    _peer_id: u32,
) -> Result<(), String> {
    // In multicast mode, connection is automatic
    Ok(())
}

/// Disconnect from all peers
#[tauri::command]
pub async fn disconnect(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.stop_discovery().await.map_err(|e| e.to_string())
}

/// Get connection status
#[tauri::command]
pub async fn get_connection_status(
    state: State<'_, Arc<AppState>>,
) -> Result<ConnectionStatus, String> {
    Ok(state.get_connection_status().await)
}

/// Start transmitting audio (PTT press)
#[tauri::command]
pub async fn start_transmit(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.start_transmit().await.map_err(|e| e.to_string())
}

/// Stop transmitting audio (PTT release)
#[tauri::command]
pub async fn stop_transmit(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.stop_transmit().await.map_err(|e| e.to_string())
}

/// Start receiving audio (already handled by discovery)
#[tauri::command]
pub async fn start_receiving(_state: State<'_, Arc<AppState>>) -> Result<(), String> {
    // Receiving is automatic in multicast mode
    Ok(())
}

/// Stop receiving audio
#[tauri::command]
pub async fn stop_receiving(_state: State<'_, Arc<AppState>>) -> Result<(), String> {
    // Receiving is automatic in multicast mode
    Ok(())
}

/// Audio devices response
#[derive(serde::Serialize)]
pub struct AudioDevices {
    pub inputs: Vec<AudioDeviceInfo>,
    pub outputs: Vec<AudioDeviceInfo>,
}

/// Get available audio devices
#[tauri::command]
pub async fn get_audio_devices(state: State<'_, Arc<AppState>>) -> Result<AudioDevices, String> {
    let (inputs, outputs) = state.get_audio_devices().await;
    Ok(AudioDevices { inputs, outputs })
}

/// Set input audio device
#[tauri::command]
pub async fn set_input_device(
    state: State<'_, Arc<AppState>>,
    device_name: String,
) -> Result<(), String> {
    state
        .set_input_device(&device_name)
        .await
        .map_err(|e| e.to_string())
}

/// Set output audio device
#[tauri::command]
pub async fn set_output_device(
    state: State<'_, Arc<AppState>>,
    device_name: String,
) -> Result<(), String> {
    state
        .set_output_device(&device_name)
        .await
        .map_err(|e| e.to_string())
}

/// Volume levels
#[derive(serde::Serialize)]
pub struct Volume {
    pub input: f32,
    pub output: f32,
}

/// Get volume levels
#[tauri::command]
pub async fn get_volume(state: State<'_, Arc<AppState>>) -> Result<Volume, String> {
    let (input, output) = state.get_volume().await;
    Ok(Volume { input, output })
}

/// Set volume levels
#[tauri::command]
pub async fn set_volume(
    state: State<'_, Arc<AppState>>,
    input: f32,
    output: f32,
) -> Result<(), String> {
    state.set_volume(input, output).await;
    Ok(())
}

/// Get current channel
#[tauri::command]
pub async fn get_channel(state: State<'_, Arc<AppState>>) -> Result<u8, String> {
    Ok(state.get_channel())
}

/// Set channel
#[tauri::command]
pub async fn set_channel(state: State<'_, Arc<AppState>>, channel: u8) -> Result<(), String> {
    state.set_channel(channel).await;
    Ok(())
}

/// Application status
#[derive(serde::Serialize)]
pub struct AppStatus {
    pub connection_status: ConnectionStatus,
    pub channel: u8,
    pub peer_count: usize,
    pub is_transmitting: bool,
    pub is_receiving: bool,
}

/// Get application status
#[tauri::command]
pub async fn get_status(state: State<'_, Arc<AppState>>) -> Result<AppStatus, String> {
    let connection_status = state.get_connection_status().await;
    let channel = state.get_channel();
    let peers = state.get_nearby_devices().await;
    let peer_count = peers.len();
    let is_receiving = state.is_receiving();

    Ok(AppStatus {
        connection_status,
        channel,
        peer_count,
        is_transmitting: matches!(connection_status, ConnectionStatus::Transmitting),
        is_receiving,
    })
}

/// Device information
#[derive(serde::Serialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_name: String,
    pub version: String,
}

/// Get device information
#[tauri::command]
pub async fn get_device_info(state: State<'_, Arc<AppState>>) -> Result<DeviceInfo, String> {
    let (device_id, device_name) = state.get_device_info();

    Ok(DeviceInfo {
        device_id: format!("{:08X}", device_id),
        device_name,
        version: crate::VERSION.to_string(),
    })
}

/// Set roger beep enabled
#[tauri::command]
pub async fn set_roger_beep(state: State<'_, Arc<AppState>>, enabled: bool) -> Result<(), String> {
    state.set_roger_beep(enabled);
    Ok(())
}

/// Set VOX enabled
#[tauri::command]
pub async fn set_vox_enabled(state: State<'_, Arc<AppState>>, enabled: bool) -> Result<(), String> {
    state.set_vox_enabled(enabled);
    Ok(())
}

/// Set VOX threshold
#[tauri::command]
pub async fn set_vox_threshold(
    state: State<'_, Arc<AppState>>,
    threshold: f32,
) -> Result<(), String> {
    state.set_vox_threshold(threshold).await;
    Ok(())
}

// ============================================================================
// Tone Commands - Audio Feedback
// ============================================================================

use crate::tones::ToneType;

/// Play connection success tone (3-tone chime)
#[tauri::command]
pub async fn play_connection_tone(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let tone_player = state.get_tone_player();
    tokio::task::spawn_blocking(move || tone_player.play_sync(ToneType::ConnectionSuccess))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Play message delivered tone (2-tone low→high)
#[tauri::command]
pub async fn play_delivered_tone(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let tone_player = state.get_tone_player();
    tokio::task::spawn_blocking(move || tone_player.play_sync(ToneType::MessageDelivered))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Play error/failed tone (2-tone mono)
#[tauri::command]
pub async fn play_failed_tone(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let tone_player = state.get_tone_player();
    tokio::task::spawn_blocking(move || tone_player.play_sync(ToneType::Failed))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Play roger beep tone
#[tauri::command]
pub async fn play_roger_tone(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let tone_player = state.get_tone_player();
    tokio::task::spawn_blocking(move || tone_player.play_sync(ToneType::RogerBeep))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

// ============================================================================
// Transport Configuration Commands
// ============================================================================

use crate::transport::TransportConfig;

/// Get transport configuration
#[tauri::command]
pub async fn get_transport_config(
    state: State<'_, Arc<AppState>>,
) -> Result<TransportConfig, String> {
    Ok(state.get_transport_config().await)
}

/// Set transport configuration
#[tauri::command]
pub async fn set_transport_config(
    state: State<'_, Arc<AppState>>,
    config: TransportConfig,
) -> Result<(), String> {
    state.set_transport_config(config).await;
    Ok(())
}

/// Network info response
#[derive(serde::Serialize)]
pub struct NetworkInfo {
    pub port: u16,
    pub multicast_addr: String,
    pub use_random_port: bool,
    pub encryption_enabled: bool,
    pub is_encrypted: bool,
    pub public_key: Option<String>,
    pub udp_audio_queue: crate::transport::QueueMetricsSnapshot,
}

/// Get network information
#[tauri::command]
pub async fn get_network_info(state: State<'_, Arc<AppState>>) -> Result<NetworkInfo, String> {
    let config = state.get_transport_config().await;
    let port = state.get_port().await;
    let is_encrypted = state.is_encrypted().await;
    let public_key = state.get_public_key().await;
    let udp_audio_queue = state.udp_queue_metrics().await;

    Ok(NetworkInfo {
        port,
        multicast_addr: config.multicast_addr,
        use_random_port: config.use_random_port,
        encryption_enabled: config.encryption_enabled,
        is_encrypted,
        public_key,
        udp_audio_queue,
    })
}

/// Set encryption enabled
#[tauri::command]
pub async fn set_encryption_enabled(
    state: State<'_, Arc<AppState>>,
    enabled: bool,
) -> Result<(), String> {
    let mut config = state.get_transport_config().await;
    config.encryption_enabled = enabled;
    state.set_transport_config(config).await;
    Ok(())
}

/// Set random port enabled
#[tauri::command]
pub async fn set_random_port_enabled(
    state: State<'_, Arc<AppState>>,
    enabled: bool,
) -> Result<(), String> {
    let mut config = state.get_transport_config().await;
    config.use_random_port = enabled;
    state.set_transport_config(config).await;
    Ok(())
}

/// Connection quality entry: (peer_id_hex, health, rtt_ms_option, transport)
/// Health: "healthy" | "degraded" | "stale"
/// Transport: "UDP" | "BLE" | "Relay"
#[tauri::command]
pub async fn get_connection_quality(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<(String, String, Option<u32>, String)>, String> {
    Ok(state.get_connection_quality().await)
}

// ============================================================================
// Cellular Relay Commands - Internet transport via the Cloudflare relay
// ============================================================================

/// Join a cellular relay session from a scanned/pasted QR JSON payload.
/// Returns the relay room id (= session_id) on success. Errors surface auth or
/// connect failures so the UI can show them.
#[tauri::command]
pub async fn join_cellular_session(
    state: State<'_, Arc<AppState>>,
    qr_json: String,
) -> Result<String, String> {
    state.join_cellular(&qr_json).await
}

/// Leave the current cellular relay session (idempotent).
#[tauri::command]
pub async fn leave_cellular_session(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.leave_cellular().await;
    Ok(())
}

/// Clear session keys, authenticated control plane, and persisted secret store.
/// Requires an in-app invoke; silent enterprise wipe still needs MDM/EMM.
#[tauri::command]
pub async fn wipe_session(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.wipe_session().await;
    Ok(())
}

/// Cellular session status as JSON (`{state,room,sent,received}`), or an empty
/// string when no session is joined.
#[tauri::command]
pub async fn get_cellular_status(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    Ok(state.cellular_status().await.unwrap_or_default())
}

/// Host of the Cloudflare relay. The invite link and the blob fetch are both
/// pinned to this host so a hostile link can't redirect the fetch elsewhere.
const RELAY_HOST: &str = "relay.sassyconsultingllc.com";

/// Process-wide HTTP client, built once. A fresh `reqwest::Client` per import
/// would throw away connection pooling and redo TLS setup each time.
static SHARE_HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();

/// Import an encrypted session invite from a share LINK
/// (`https://relay.sassyconsultingllc.com/v/<id>#<base64url-key>`): fetch the
/// opaque blob, decrypt it with the key carried in the URL fragment via the
/// shared core, then join exactly like a pasted QR. Returns the relay room id.
///
/// The decryption key lives ONLY in the `#fragment`, which is never transmitted
/// to the relay — the worker stores ciphertext it cannot read. This is the
/// desktop counterpart to Android's `SessionShareLink.importFromShareUri` and
/// reuses the same `sassytalkie_core::share` decrypt as every other platform.
#[tauri::command]
pub async fn import_share_link(
    state: State<'_, Arc<AppState>>,
    url: String,
) -> Result<String, String> {
    let qr_json = fetch_and_decrypt_share(&url).await?;
    state.join_cellular(&qr_json).await
}

/// Resolve a `/v/<id>#<key>` link to the decrypted session QR JSON. Split out
/// from the command so the fetch+decrypt logic stays free of Tauri state.
async fn fetch_and_decrypt_share(link: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(link).map_err(|_| "Malformed link".to_string())?;
    if parsed.scheme() != "https" || parsed.host_str() != Some(RELAY_HOST) {
        return Err("Not a SassyTalk invite link".to_string());
    }
    let id = parsed
        .path()
        .strip_prefix("/v/")
        .ok_or_else(|| "Not a share link".to_string())?;
    if !sassytalkie_core::share::is_valid_share_id(id) {
        return Err("Malformed share link".to_string());
    }
    let key_b64url = parsed
        .fragment()
        .ok_or_else(|| "Missing decryption key in URL fragment".to_string())?;

    let resp = SHARE_HTTP
        .get_or_init(reqwest::Client::new)
        .get(format!("https://{RELAY_HOST}/share/{id}"))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    match resp.status().as_u16() {
        404 => return Err("Invite already used or expired".to_string()),
        429 => return Err("Too many requests; try later".to_string()),
        code if !(200..300).contains(&code) => {
            return Err(format!("Server returned HTTP {code}"));
        }
        _ => {}
    }
    let blob = resp
        .bytes()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    sassytalkie_core::share::decrypt_share_blob(&blob, key_b64url)
}
