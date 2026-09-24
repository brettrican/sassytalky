// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-A5NIGVWXYNXL
package com.sassyconsulting.sassytalkie

import android.net.Uri
import android.util.Base64
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.json.JSONObject
import java.security.SecureRandom
import java.util.concurrent.TimeUnit
import javax.crypto.Cipher
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec

/**
 * Client for the encrypted session-share endpoint.
 *
 * Wire protocol — MUST stay in lockstep with cloudflare-worker/src/share.js:
 *   1. Pull the relay room id (= session_id) out of the QR JSON.
 *   2. GET  /auth?room=<room>                       → { token }  (room capability)
 *   3. Generate a fresh AES-256 key + 12-byte IV; AES-GCM encrypt the QR JSON.
 *   4. POST /share?room=<room>[&ttl=][&burn=1]      body = IV‖ciphertext (raw bytes)
 *           Authorization: Bearer <token>           → { id, expires_at }
 *   5. Build URL: https://relay.sassyconsultingllc.com/v/<id>#<base64url-key>
 *
 * The blob the relay stores is opaque: it's the IV prepended to the AES-GCM
 * ciphertext. The decryption key lives ONLY in the URL #fragment, which browsers
 * and apps never transmit to the server — so a KV dump is useless, and the relay
 * only ever sees ciphertext.
 *
 * A previous revision spoke a `/share → {token}`, `/s/<token> → {c,i}` protocol
 * that the worker never implemented (it always served `/share/<id>` returning a
 * raw octet-stream and a JSON `{id}`). That mismatch made "Copy Link" fail with
 * "Server returned no token" and made tapped links never establish a session.
 */
object SessionShareLink {

    const val RELAY_BASE = "https://relay.sassyconsultingllc.com"
    /**
     * Custom scheme that opens the app directly, no App Links verification
     * required. Primary scheme for newly minted invites.
     */
    const val APP_SCHEME = "sassy-talks"

    /**
     * Previous scheme, still accepted on the way IN.
     *
     * Invites get pasted into chats and left there — a link shared last week is
     * a live invite until it expires, and dropping this scheme would turn every
     * one of them into a dead tap with no explanation. Accepting both costs one
     * comparison, and the manifest registers both filters for the same reason.
     * New links are minted with [APP_SCHEME] only.
     */
    const val LEGACY_APP_SCHEME = "sassytalk"

    /** True for either the current or the legacy app scheme. */
    fun isAppScheme(scheme: String?): Boolean =
        scheme == APP_SCHEME || scheme == LEGACY_APP_SCHEME

    private const val GCM_TAG_BITS = 128
    private const val IV_LEN = 12
    private val JSON_MEDIA = "application/json".toMediaType()
    private val OCTET_MEDIA = "application/octet-stream".toMediaType()
    // The worker's share id is base64url, 16–64 chars (see share.js ID_RE).
    private val SHARE_ID_RE = Regex("^[A-Za-z0-9_-]{16,64}$")

    private val http: OkHttpClient by lazy {
        RelayTlsPins.apply(
            OkHttpClient.Builder()
                .connectTimeout(10, TimeUnit.SECONDS)
                .readTimeout(15, TimeUnit.SECONDS),
        ).build()
    }

    sealed class Result {
        /**
         * [url] / [expiresAt] / [ttlSec] are populated for [createShare].
         * [json] is populated for [importFromShareUri] and contains the
         * decrypted session payload ready to hand to [SassyTalkNative.importSession].
         */
        data class Ok(
            /** `sassytalk://v/<id>#<key>` — opens the installed app directly. */
            val url: String = "",
            /** `https://relay…/v/<id>#<key>` — web fallback / Universal Link. */
            val httpsUrl: String = "",
            val expiresAt: Long = 0L,
            val ttlSec: Int = 0,
            val json: String = "",
        ) : Result()
        data class Err(val message: String) : Result()
    }

    /**
     * Encrypt [sessionJson] and upload it as a one-shot invite blob. Blocks —
     * call from a coroutine on Dispatchers.IO, or wrap in
     * [kotlinx.coroutines.withContext].
     *
     * @param ttlSec optional server-side expiry; null lets the relay apply its
     *   default (7 days). The blob is also burned on first read when [burn].
     * @param burn one-time dead-drop semantics: the relay deletes the blob the
     *   first time it is fetched. Defaults true to match the "one-time link" UX.
     */
    fun createShare(sessionJson: String, ttlSec: Int? = null, burn: Boolean = true): Result {
        if (sessionJson.isEmpty()) return Result.Err("Empty session payload")

        // The relay binds every share blob to a room and requires a capability
        // token for it. The QR JSON is the serialized SessionKey, whose
        // session_id IS the relay room id.
        val roomId = try {
            JSONObject(sessionJson).optString("session_id")
        } catch (t: Throwable) {
            return Result.Err("Malformed session payload")
        }
        if (roomId.isBlank()) return Result.Err("Session has no room id")

        val token = when (val t = fetchRoomToken(roomId)) {
            is TokenResult.Ok -> t.token
            is TokenResult.Err -> return Result.Err(t.message)
        }

        val keyBytes = ByteArray(32)
        SecureRandom().nextBytes(keyBytes)
        val iv = ByteArray(IV_LEN)
        SecureRandom().nextBytes(iv)

        // blob = IV ‖ (ciphertext+GCM tag). The IV travels with the ciphertext
        // because the relay hands back exactly these bytes on GET; only the key
        // is held back in the URL fragment.
        val blob: ByteArray = try {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(
                Cipher.ENCRYPT_MODE,
                SecretKeySpec(keyBytes, "AES"),
                GCMParameterSpec(GCM_TAG_BITS, iv),
            )
            iv + cipher.doFinal(sessionJson.toByteArray(Charsets.UTF_8))
        } catch (t: Throwable) {
            keyBytes.fill(0)
            return Result.Err("Encrypt failed: ${t.message}")
        }

        val postUrl = buildString {
            append(RELAY_BASE).append("/share?room=").append(Uri.encode(roomId))
            if (ttlSec != null) append("&ttl=").append(ttlSec)
            if (burn) append("&burn=1")
        }

        val req = Request.Builder()
            .url(postUrl)
            .header("Authorization", "Bearer $token")
            .post(blob.toRequestBody(OCTET_MEDIA))
            .build()

        val (id, expiresAt) = try {
            http.newCall(req).execute().use { resp ->
                if (!resp.isSuccessful) {
                    keyBytes.fill(0)
                    return Result.Err("Server returned HTTP ${resp.code}")
                }
                val json = JSONObject(resp.body?.string() ?: "")
                json.optString("id") to json.optLong("expires_at", 0L)
            }
        } catch (t: Throwable) {
            keyBytes.fill(0)
            return Result.Err("Network error: ${t.message}")
        }

        if (id.isNullOrBlank()) {
            keyBytes.fill(0)
            return Result.Err("Server returned no share id")
        }

        val keyB64Url = Base64.encodeToString(
            keyBytes,
            Base64.NO_PADDING or Base64.NO_WRAP or Base64.URL_SAFE,
        )
        // Zero the key bytes — they're now in the URL, no reason to keep them
        // in memory longer than needed.
        keyBytes.fill(0)

        val effectiveTtl = if (ttlSec != null) ttlSec
        else if (expiresAt > 0L) ((expiresAt - System.currentTimeMillis() / 1000L)).toInt()
        else 0

        val httpsUrl = "$RELAY_BASE/v/$id#$keyB64Url"
        return Result.Ok(
            url = buildAppLink(id, keyB64Url),
            httpsUrl = httpsUrl,
            expiresAt = expiresAt,
            ttlSec = effectiveTtl,
        )
    }

    /** `sassytalk://v/<id>#<key>` — registered custom-scheme deep link. */
    fun buildAppLink(shareId: String, keyB64Url: String): String =
        "$APP_SCHEME://v/$shareId#$keyB64Url"

    /** True when [text] looks like an encrypted invite link (not raw QR JSON). */
    fun looksLikeShareLink(text: String): Boolean {
        val trimmed = text.trim()
        return try {
            val uri = Uri.parse(trimmed)
            isInviteUri(uri)
        } catch (_: Throwable) {
            false
        }
    }

    private fun isInviteUri(uri: Uri): Boolean {
        if (isAppScheme(uri.scheme) && uri.host == "v") {
            val id = (uri.path ?: "").removePrefix("/")
            return SHARE_ID_RE.matches(id)
        }
        return uri.scheme == "https" &&
            uri.host == "relay.sassyconsultingllc.com" &&
            (uri.path ?: "").startsWith("/v/")
    }

    private fun parseInviteParts(uri: Uri): Pair<String, String>? {
        val id = when {
            isAppScheme(uri.scheme) && uri.host == "v" ->
                (uri.path ?: "").removePrefix("/")
            uri.scheme == "https" && uri.host == "relay.sassyconsultingllc.com" ->
                (uri.path ?: "").removePrefix("/v/")
            else -> return null
        }
        if (!SHARE_ID_RE.matches(id)) return null
        val key = uri.fragment?.takeIf { it.isNotEmpty() } ?: return null
        return id to key
    }

    /**
     * Parse a pasted invite URL string. Convenience wrapper around
     * [importFromShareUri] for the Enter Code tab and clipboard paste paths.
     */
    fun importFromShareText(text: String): Result {
        // A pasted invite can arrive with surrounding text or a second line
        // (older builds copied "sassytalk://…\nhttps://…"). Pull the first token
        // that is a real invite URI so an annotated / multi-line paste still
        // resolves, instead of failing Base64 on a newline-polluted fragment.
        val token = text.split(Regex("\\s+")).map { it.trim() }
            .firstOrNull { looksLikeShareLink(it) } ?: text.trim()
        return importFromShareUri(Uri.parse(token))
    }

    /**
     * Resolve a deep-link Uri to a decrypted session JSON, ready to feed into
     * [SassyTalkNative.importSession]. Blocking — call from a coroutine.
     *
     * Only accepts the encrypted https /v/<id>#<key> form. A previous draft
     * supported sassytalk://join#<urlencoded-json>, but that put plaintext
     * session JSON (including the AES-256 room key) into any URL the user
     * tapped — a phishing one-tap force-join into an attacker-controlled
     * session. Removed.
     */
    fun importFromShareUri(uri: Uri): Result {
        if (!isInviteUri(uri)) return Result.Err("Unrecognized share URL")
        // A missing #fragment is the "key was stripped" case (some launchers and
        // link handlers drop it). Report it distinctly — AppNavigation matches
        // the word "fragment" to show the paste-the-whole-link recovery and
        // bounce to Enter Code — instead of the generic "Malformed share link".
        if (uri.fragment.isNullOrEmpty()) {
            return Result.Err("Invite link is missing its key fragment")
        }
        val (id, keyB64) = parseInviteParts(uri)
            ?: return Result.Err("Malformed share link")

        val keyBytes = try {
                Base64.decode(keyB64, Base64.URL_SAFE or Base64.NO_PADDING or Base64.NO_WRAP)
            } catch (t: Throwable) {
                return Result.Err("Invalid key encoding")
            }
            // Single wipe point: every exit below (including non-local returns
            // from inside `use {}`) runs the finally, so the AES key is always
            // cleared from the heap without a fill(0) scattered at each return.
            try {
                if (keyBytes.size != 32) return Result.Err("Key must be 32 bytes")

                val fetchReq = Request.Builder()
                    .url("$RELAY_BASE/share/$id")
                    .get()
                    .build()

                val blob: ByteArray = try {
                    http.newCall(fetchReq).execute().use { resp ->
                        if (resp.code == 404) return Result.Err("Invite already used or expired")
                        if (resp.code == 429) return Result.Err("Too many requests; try later")
                        if (!resp.isSuccessful) return Result.Err("Server returned HTTP ${resp.code}")
                        resp.body?.bytes() ?: ByteArray(0)
                    }
                } catch (t: Throwable) {
                    return Result.Err("Network error: ${t.message}")
                }
                // blob = IV ‖ ciphertext+tag. Need at least IV + a GCM tag (16 bytes).
                if (blob.size <= IV_LEN + 16) {
                    return Result.Err("Server response missing ciphertext")
                }

                val plain = try {
                    val iv = blob.copyOfRange(0, IV_LEN)
                    val ct = blob.copyOfRange(IV_LEN, blob.size)
                    val cipher = Cipher.getInstance("AES/GCM/NoPadding")
                    cipher.init(
                        Cipher.DECRYPT_MODE,
                        SecretKeySpec(keyBytes, "AES"),
                        GCMParameterSpec(GCM_TAG_BITS, iv),
                    )
                    String(cipher.doFinal(ct), Charsets.UTF_8)
                } catch (t: Throwable) {
                    return Result.Err("Decryption failed (key wrong or payload tampered)")
                }

                return Result.Ok(url = uri.toString(), httpsUrl = "", json = plain)
            } finally {
                keyBytes.fill(0)
            }
    }

    private sealed class TokenResult {
        data class Ok(val token: String) : TokenResult()
        data class Err(val message: String) : TokenResult()
    }

    /**
     * Mint a room capability token from the relay's /auth endpoint — the same
     * grant used to open the WebSocket / register presence, and the one
     * /share POST requires. Blocking.
     *
     * Legacy room-only token (no peer=): share mint runs before we always have
     * a peer id wired here; old clients still accept room-bound tokens and
     * REQUIRE_AUTH_PROOF stays off so this path keeps working.
     */
    private fun fetchRoomToken(roomId: String): TokenResult {
        val req = Request.Builder()
            .url("$RELAY_BASE/auth?room=${Uri.encode(roomId)}")
            .get()
            .build()
        return try {
            http.newCall(req).execute().use { resp ->
                if (!resp.isSuccessful) return TokenResult.Err("Auth failed: HTTP ${resp.code}")
                val token = JSONObject(resp.body?.string() ?: "").optString("token")
                if (token.isBlank()) TokenResult.Err("Relay issued no token")
                else TokenResult.Ok(token)
            }
        } catch (t: Throwable) {
            TokenResult.Err("Network error: ${t.message}")
        }
    }
}
