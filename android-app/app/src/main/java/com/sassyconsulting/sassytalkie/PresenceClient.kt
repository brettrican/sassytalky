// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-GJTGG7M3YYW5
package com.sassyconsulting.sassytalkie

import android.content.Context
import android.net.Uri
import android.util.Log
import com.google.firebase.messaging.FirebaseMessaging
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.json.JSONObject
import java.util.concurrent.TimeUnit

/**
 * Thin client for the relay's /presence endpoint. Lets the app register its
 * FCM token under (current room, this install's peer_id) so the relay can
 * fire a wake push when audio arrives for a peer whose WS has dropped.
 *
 * Stateless / static — fire and forget. Callers should run from a background
 * coroutine on Dispatchers.IO.
 *
 * Auth: POST/DELETE require Authorization: Bearer <room capability>, the same
 * grant CellularWebSocketClient mints from /auth (with peer= when known).
 */
object PresenceClient {
    private const val TAG = "PresenceClient"

    private val http: OkHttpClient by lazy {
        RelayTlsPins.apply(
            OkHttpClient.Builder()
                .connectTimeout(10, TimeUnit.SECONDS)
                .readTimeout(15, TimeUnit.SECONDS),
        ).build()
    }

    private val JSON_MEDIA = "application/json".toMediaType()

    /** Returns true on 2xx, false otherwise. Logs failures but never throws. */
    fun upload(context: Context, roomId: String, fcmToken: String): Boolean {
        if (roomId.isBlank() || fcmToken.isBlank()) return false
        val peerId = InstallId.get(context)
        val cap = fetchCapabilityToken(roomId, peerId) ?: return false
        val body = JSONObject().apply {
            put("room", roomId)
            put("peer", peerId)
            put("token", fcmToken)
        }
        val req = Request.Builder()
            .url("${SessionShareLink.RELAY_BASE}/presence")
            .header("Authorization", "Bearer $cap")
            .post(body.toString().toRequestBody(JSON_MEDIA))
            .build()
        return try {
            http.newCall(req).execute().use { r ->
                if (!r.isSuccessful) {
                    Log.w(TAG, "presence upload failed: HTTP ${r.code}")
                }
                r.isSuccessful
            }
        } catch (t: Throwable) {
            Log.w(TAG, "presence upload exception: ${t.message}")
            false
        }
    }

    /**
     * Fetch the current FCM registration token and upload it to /presence
     * for [roomId]. No-op when FCM hasn't been configured (no
     * google-services.json) — the token fetch will throw and we just log.
     *
     * Fire-and-forget: returns immediately, work happens on FCM's background
     * thread + an OkHttp dispatcher thread.
     */
    fun uploadCurrentToken(context: Context, roomId: String) {
        if (roomId.isBlank()) return
        val appCtx = context.applicationContext
        try {
            FirebaseMessaging.getInstance().token
                .addOnSuccessListener { token ->
                    if (!token.isNullOrBlank()) {
                        Thread {
                            upload(appCtx, roomId, token)
                        }.apply { name = "presence-upload"; isDaemon = true }.start()
                    }
                }
                .addOnFailureListener { e ->
                    Log.w(TAG, "FCM token fetch failed: ${e.message}")
                }
        } catch (t: Throwable) {
            // FirebaseApp not initialized (no google-services.json) — safe skip.
            Log.d(TAG, "FCM not configured, skipping presence upload: ${t.message}")
        }
    }

    /** DELETE the (room, peer) row — call on session clear / sign out. */
    fun remove(context: Context, roomId: String): Boolean {
        if (roomId.isBlank()) return false
        val peerId = InstallId.get(context)
        val cap = fetchCapabilityToken(roomId, peerId) ?: return false
        val body = JSONObject().apply {
            put("room", roomId)
            put("peer", peerId)
        }
        val req = Request.Builder()
            .url("${SessionShareLink.RELAY_BASE}/presence")
            .header("Authorization", "Bearer $cap")
            .delete(body.toString().toRequestBody(JSON_MEDIA))
            .build()
        return try {
            http.newCall(req).execute().use { r -> r.isSuccessful }
        } catch (t: Throwable) {
            Log.w(TAG, "presence remove exception: ${t.message}")
            false
        }
    }

    /**
     * Mint a room capability (with peer= so presence identity matches).
     * Never logs the token value.
     */
    private fun fetchCapabilityToken(roomId: String, peerId: String): String? {
        val url = Uri.parse("${SessionShareLink.RELAY_BASE}/auth").buildUpon()
            .appendQueryParameter("room", roomId)
            .appendQueryParameter("peer", peerId)
            .build()
            .toString()
        val req = Request.Builder().url(url).get().build()
        return try {
            http.newCall(req).execute().use { resp ->
                if (!resp.isSuccessful) {
                    Log.w(TAG, "presence auth failed: HTTP ${resp.code}")
                    return null
                }
                val token = JSONObject(resp.body?.string() ?: "").optString("token")
                if (token.isBlank()) {
                    Log.w(TAG, "presence auth returned empty token")
                    null
                } else {
                    token
                }
            }
        } catch (t: Throwable) {
            Log.w(TAG, "presence auth exception: ${t.message}")
            null
        }
    }
}
