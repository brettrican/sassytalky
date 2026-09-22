// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-RQIENYDNOL5R
//! share — open encrypted session-invite blobs from the relay's `/share/<id>`
//! endpoint, the server-side half of an invite link
//! `https://relay.sassyconsultingllc.com/v/<id>#<base64url-key>`.
//!
//! The flow is identical on every platform; only step 3 lives here:
//!   1. (platform) Parse the link → share id + url-safe base64 key fragment.
//!   2. (platform) HTTP GET `/share/<id>` → the opaque blob bytes.
//!   3. (core)     [`decrypt_share_blob`] → the session QR JSON.
//!   4. (platform) Feed that JSON into the normal QR-import path
//!                 (`SessionManager::import_session` and friends).
//!
//! Wire format — MUST stay byte-identical with the encrypting side
//! (android-app `SessionShareLink.kt`, and any future host):
//!   key  = 32-byte AES-256 key, url-safe base64, no padding (URL `#fragment`)
//!   blob = `IV(12) ‖ AES-256-GCM(ciphertext ‖ 16-byte tag)`, no AAD
//!
//! This is a STANDALONE one-shot AES-GCM open with a random 12-byte IV — it is
//! deliberately NOT [`crate::crypto::CryptoSession`], whose counter-based nonces
//! and replay window model a long-lived audio stream, not a single sealed
//! invite. Keeping it separate means all three platforms decrypt invites through
//! one audited function instead of three hand-rolled AES-GCM call sites.

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use base64::Engine;
use zeroize::Zeroizing;

/// 96-bit GCM nonce, matching the encrypters' 12-byte random IV.
const IV_LEN: usize = 12;
/// AES-256-GCM authentication tag length (128-bit).
const TAG_LEN: usize = 16;

/// True if `id` is a well-formed relay share id — url-safe base64, 16–64 chars
/// (the worker's `share.js` `ID_RE`). The canonical validator: platform code
/// calls this instead of hand-rolling the alphabet/length so they can't drift.
pub fn is_valid_share_id(id: &str) -> bool {
    (16..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Decrypt a session-share blob fetched from `/share/<id>` into the plaintext
/// session QR JSON.
///
/// * `blob` — the raw bytes the relay returned: `IV(12) ‖ ciphertext+tag`.
/// * `key_b64url` — the URL `#fragment`: a 32-byte AES-256 key, url-safe base64.
///   Padding is tolerated but not required (the host emits none).
///
/// Errors are intentionally coarse — a single "decryption failed" covers both a
/// wrong key and a tampered blob so the caller can't be turned into an oracle
/// that distinguishes the two.
pub fn decrypt_share_blob(blob: &[u8], key_b64url: &str) -> Result<String, String> {
    // url-safe, padding-indifferent: strip any '=' the host might have added and
    // decode with the no-pad alphabet so both forms work.
    let cleaned = key_b64url.trim().trim_end_matches('=');
    let key_bytes = Zeroizing::new(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(cleaned)
            .map_err(|_| "Invalid key encoding".to_string())?,
    );
    if key_bytes.len() != 32 {
        return Err("Key must be 32 bytes".to_string());
    }
    // Need at least the IV plus one GCM tag, else there is no ciphertext to open.
    if blob.len() <= IV_LEN + TAG_LEN {
        return Err("Blob too short".to_string());
    }

    let (iv, ct) = blob.split_at(IV_LEN);
    let key = Key::<Aes256Gcm>::from_slice(key_bytes.as_slice());
    let cipher = Aes256Gcm::new(key);
    let nonce = Nonce::from_slice(iv);

    let plaintext = cipher
        .decrypt(nonce, ct)
        .map_err(|_| "Decryption failed (key wrong or payload tampered)".to_string())?;

    // The decrypted session JSON is the return value, so it can't be zeroized
    // anyway — move it straight into the String rather than cloning out of a
    // Zeroizing wrapper. (The key bytes, which DO stay secret, remain zeroized.)
    String::from_utf8(plaintext).map_err(|_| "Decrypted payload is not valid UTF-8".to_string())
}

/// Seal session-QR JSON into a share blob. Inverse of [`decrypt_share_blob`].
///
/// Returns `(blob, key_b64url)` where `blob` is `IV(12) ‖ ciphertext+tag` and
/// `key_b64url` is the 32-byte key as url-safe base64 with no padding — the
/// fragment an invite link carries. The key is random; callers must put it in
/// the URL and nowhere else (the relay stores only `blob`).
///
/// iOS hosts mint invites through this so the bytes Android's JCA encrypter
/// produces stay interchangeable. Do not reimplement AES-GCM in Swift.
pub fn encrypt_share_blob(json: &str) -> Result<(Vec<u8>, String), String> {
    use rand::RngCore;
    if json.is_empty() {
        return Err("Empty session payload".to_string());
    }
    let mut key = Zeroizing::new([0u8; 32]);
    let mut iv = [0u8; IV_LEN];
    rand::thread_rng().fill_bytes(key.as_mut());
    rand::thread_rng().fill_bytes(&mut iv);

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key.as_slice()));
    let ct = cipher
        .encrypt(Nonce::from_slice(&iv), json.as_bytes())
        .map_err(|_| "Encrypt failed".to_string())?;
    let mut blob = Vec::with_capacity(IV_LEN + ct.len());
    blob.extend_from_slice(&iv);
    blob.extend_from_slice(&ct);
    let key_b64url = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.as_slice());
    Ok((blob, key_b64url))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes_gcm::aead::Aead;
    use base64::Engine;

    /// Encrypt exactly the way the host does: `IV(12) ‖ ciphertext+tag`, no AAD,
    /// and return `(blob, key_b64url)` so the test mirrors the real wire.
    fn seal(plaintext: &[u8], key: &[u8; 32], iv: &[u8; 12]) -> (Vec<u8>, String) {
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
        let ct = cipher
            .encrypt(Nonce::from_slice(iv), plaintext)
            .expect("seal");
        let mut blob = Vec::with_capacity(iv.len() + ct.len());
        blob.extend_from_slice(iv);
        blob.extend_from_slice(&ct);
        let key_b64url = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key);
        (blob, key_b64url)
    }

    #[test]
    fn round_trips_session_json() {
        let key = [7u8; 32];
        let iv = [3u8; 12];
        let json = r#"{"session_id":"abc","channel":1,"key":"deadbeef"}"#;
        let (blob, key_b64url) = seal(json.as_bytes(), &key, &iv);
        assert_eq!(decrypt_share_blob(&blob, &key_b64url).unwrap(), json);
    }

    #[test]
    fn public_encrypt_round_trips_through_decrypt() {
        let json = r#"{"session_id":"room-1","channel":2,"key":"abcd"}"#;
        let (blob, key_b64url) = encrypt_share_blob(json).unwrap();
        assert!(blob.len() > IV_LEN + TAG_LEN);
        assert_eq!(decrypt_share_blob(&blob, &key_b64url).unwrap(), json);
    }

    #[test]
    fn public_encrypt_rejects_empty() {
        assert!(encrypt_share_blob("").is_err());
    }

    #[test]
    fn tolerates_base64_padding_in_fragment() {
        let key = [9u8; 32];
        let iv = [1u8; 12];
        let (blob, key_b64url) = seal(b"hello", &key, &iv);
        // Force standard base64 *with* padding and feed it in — must still work.
        let padded = base64::engine::general_purpose::URL_SAFE.encode(key);
        assert!(padded.ends_with('='));
        assert_eq!(decrypt_share_blob(&blob, &padded).unwrap(), "hello");
        // And the canonical no-pad form too.
        assert_eq!(decrypt_share_blob(&blob, &key_b64url).unwrap(), "hello");
    }

    #[test]
    fn rejects_wrong_key() {
        let (blob, _) = seal(b"secret", &[7u8; 32], &[3u8; 12]);
        let wrong = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([8u8; 32]);
        assert!(decrypt_share_blob(&blob, &wrong).is_err());
    }

    #[test]
    fn rejects_tampered_ciphertext() {
        let (mut blob, key_b64url) = seal(b"secret", &[7u8; 32], &[3u8; 12]);
        let last = blob.len() - 1;
        blob[last] ^= 0xff; // flip a tag byte
        assert!(decrypt_share_blob(&blob, &key_b64url).is_err());
    }

    #[test]
    fn rejects_bad_key_length() {
        let (blob, _) = seal(b"secret", &[7u8; 32], &[3u8; 12]);
        let short = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0u8; 16]);
        assert_eq!(
            decrypt_share_blob(&blob, &short).unwrap_err(),
            "Key must be 32 bytes"
        );
    }

    #[test]
    fn share_id_validation() {
        assert!(is_valid_share_id("AbCd1234EfGh5678")); // 16 chars
        assert!(is_valid_share_id(&"a".repeat(64)));
        assert!(is_valid_share_id("aaaaaaaaaaaaaaaa_-")); // url-safe extras
        assert!(!is_valid_share_id("short")); // < 16
        assert!(!is_valid_share_id(&"a".repeat(65))); // > 64
        assert!(!is_valid_share_id("AbCd1234EfGh5678/../x")); // bad chars
    }

    #[test]
    fn rejects_short_blob() {
        let key_b64url = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7u8; 32]);
        assert_eq!(
            decrypt_share_blob(&[0u8; 20], &key_b64url).unwrap_err(),
            "Blob too short"
        );
    }

    /// Known-answer test from an INDEPENDENT implementation (Node's OpenSSL
    /// AES-256-GCM). Proves core decrypts blobs produced the way Android's Java
    /// JCA — and any standard GCM encrypter — produces them (`IV ‖ ct ‖ tag`,
    /// no AAD): genuine cross-platform parity, not just a Rust self-round-trip.
    #[test]
    fn decrypts_independent_openssl_vector() {
        fn unhex(s: &str) -> Vec<u8> {
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
                .collect()
        }
        let blob = unhex(
            "a0a1a2a3a4a5a6a7a8a9aaab9d3a0f4836b86bd00c3aeeb72540e2b841c96b74a1d4764e\
             b02c45ee1ec51b64be547dcc8300385826be3eea582fc9bd155e100f40fc381933317e2e\
             fb1ee4ddd19ebf4d72d785969bc082fe5564ce8a9eb406787e053a8c1f1f43",
        );
        let key_b64url = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";
        let expected =
            r#"{"session_id":"f1e2d3c4","channel":3,"key":"QUJDREVG","group_name":"Bravo"}"#;
        assert_eq!(decrypt_share_blob(&blob, key_b64url).unwrap(), expected);
    }

    #[test]
    fn rejects_bad_base64_key() {
        let (blob, _) = seal(b"secret", &[7u8; 32], &[3u8; 12]);
        assert_eq!(
            decrypt_share_blob(&blob, "not valid base64!!!").unwrap_err(),
            "Invalid key encoding"
        );
    }
}
