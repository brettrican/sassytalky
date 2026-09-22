// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-FQRXO5OJTPWD
package com.sassyconsulting.sassytalkie

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.SharedPreferences
import android.os.Build
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch

/**
 * Receives wake-pushes from the relay. The relay fires a data-only,
 * priority=HIGH FCM message of the form:
 *
 *   data: { kind|type: "wake", room: "<room_id>", ts|sentAt: "<senderMs>" }
 *
 * Our job:
 *   1. Bring WalkieService into the foreground if it isn't already (relay WS
 *      lives on that service).
 *   2. Tell the service to re-attempt cellular relay attach for the room.
 *   3. Persist the room hint so the service handles the wake even if it has
 *      to cold-start (rare, but FCM can land on a fully-killed process).
 *
 * Also handles FCM token rotation: on onNewToken, re-upload to /presence so
 * the relay's stored token stays fresh.
 */
class SassyTalkFcmService : FirebaseMessagingService() {

    companion object {
        private const val TAG = "SassyTalkFcm"
        // Encrypted store filename. Deliberately NOT the old plaintext name so
        // EncryptedSharedPreferences never tries to open a pre-existing
        // plaintext file of the same name (that throws).
        const val WAKE_PREFS = "sassy_wake_state_enc"
        private const val LEGACY_WAKE_PREFS = "sassy_wake_state"
        const val WAKE_KEY_LAST_TOKEN = "fcm_token"
        const val WAKE_KEY_LAST_ROOM = "last_wake_room"
        const val WAKE_KEY_LAST_TS = "last_wake_ts"
    }

    private val scope = CoroutineScope(Dispatchers.IO + SupervisorJob())

    override fun onDestroy() {
        super.onDestroy()
        // Don't leak the IO scope past the service lifecycle.
        scope.cancel()
    }

    /**
     * Keystore-backed prefs for the FCM token + wake hints. The token addresses
     * wake-pushes to this device, so it must not sit in a plaintext file
     * (readable on a rooted device). Returns null if Keystore is unavailable —
     * these are write-only hints, so skipping a write is harmless.
     */
    private fun wakePrefs(): SharedPreferences? = try {
        // One-time cleanup of any legacy plaintext file from older builds.
        applicationContext.deleteSharedPreferences(LEGACY_WAKE_PREFS)
        val masterKey = MasterKey.Builder(applicationContext)
            .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
            .build()
        EncryptedSharedPreferences.create(
            applicationContext,
            WAKE_PREFS,
            masterKey,
            EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
            EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
        )
    } catch (e: Exception) {
        Log.w(TAG, "Encrypted wake prefs unavailable; not persisting: ${e.message}")
        null
    }

    override fun onNewToken(token: String) {
        try {
            super.onNewToken(token)
            persistAndUploadToken(token)
        } catch (t: Throwable) {
            Log.w(TAG, "FCM onNewToken failed closed (no crash): ${t.message}")
        }
    }

    private fun persistAndUploadToken(token: String) {
        Log.i(TAG, "FCM token refreshed (len=${token.length})")
        wakePrefs()?.edit()
            ?.putString(WAKE_KEY_LAST_TOKEN, token)
            ?.apply()

        // Re-upload to /presence if we know the current room.
        val roomId = currentRoomId()
        if (!roomId.isNullOrBlank()) {
            scope.launch {
                PresenceClient.upload(applicationContext, roomId, token)
            }
        }
    }

    override fun onMessageReceived(message: RemoteMessage) {
        try {
            super.onMessageReceived(message)
            handleWakeMessage(message)
        } catch (t: Throwable) {
            Log.w(TAG, "FCM wake handler failed closed (no crash): ${t.message}")
        }
    }

    private fun handleWakeMessage(message: RemoteMessage) {
        val data = message.data
        // Accept kind (Android historical) or type (older relay) as the wake marker.
        val kind = data["kind"] ?: data["type"]
        if (kind != "wake") {
            Log.d(TAG, "FCM message kind=$kind — ignoring")
            return
        }
        val room = data["room"] ?: ""
        val wakeTs = data["ts"] ?: data["sentAt"]
        Log.i(TAG, "WAKE push received room=$room ts=$wakeTs")

        // Persist so a cold-started service can act on it.
        wakePrefs()?.edit()
            ?.putString(WAKE_KEY_LAST_ROOM, room)
            ?.putLong(WAKE_KEY_LAST_TS, System.currentTimeMillis())
            ?.apply()

        // Prefer nudging the already-running WalkieService (sticky radio FGS)
        // without yanking the UI. Cold-start MainActivity only when the service
        // was not alive — required to bootstrap PttCoordinator from a killed
        // process (A14+ blocks startForegroundService+microphone from FCM alone).
        val serviceWasRunning = WalkieService.isRunning
        when (FcmWakePolicy.path(serviceWasRunning)) {
            FcmWakePolicy.Path.WarmService -> {
                val serviceIntent = Intent(applicationContext, WalkieService::class.java).apply {
                    action = WalkieService.ACTION_WAKE
                    putExtra(WalkieService.EXTRA_ROOM, room)
                }
                try {
                    applicationContext.startService(serviceIntent)
                    Log.i(TAG, "WAKE → WalkieService ACTION_WAKE room=$room (warm)")
                } catch (t: Throwable) {
                    Log.d(TAG, "warm-path startService skipped: ${t.message}")
                    postWakeNotification(room)
                }
            }
            FcmWakePolicy.Path.VisibleNotification -> {
                // API 34+ blocks microphone FGS from FCM. Do not startActivity
                // from the background either (API 29+). A high-priority
                // notification is the allowed bootstrap; the user tap opens
                // MainActivity which then starts the radio FGS legally.
                Log.i(TAG, "WAKE cold → visible notification room=$room")
                postWakeNotification(room)
            }
        }
    }

    private fun postWakeNotification(room: String) {
        val nm = getSystemService(NotificationManager::class.java) ?: return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                FcmWakePolicy.WAKE_CHANNEL_ID,
                "Radio wake",
                FcmWakePolicy.wakeChannelImportance(),
            ).apply {
                description = "Tap to reconnect the radio after a wake push"
                setShowBadge(true)
            }
            nm.createNotificationChannel(channel)
        }
        val launch = Intent(applicationContext, MainActivity::class.java).apply {
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
            putExtra(WalkieService.EXTRA_ROOM, room)
        }
        val pi = PendingIntent.getActivity(
            applicationContext,
            41,
            launch,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val n = NotificationCompat.Builder(this, FcmWakePolicy.WAKE_CHANNEL_ID)
            .setSmallIcon(android.R.drawable.ic_btn_speak_now)
            .setContentTitle("Sassy-Talk")
            .setContentText("Radio wake — tap to reconnect")
            .setContentIntent(pi)
            .setAutoCancel(true)
            .setOngoing(false)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .setPublicVersion(
                NotificationCompat.Builder(this, FcmWakePolicy.WAKE_CHANNEL_ID)
                    .setSmallIcon(android.R.drawable.ic_btn_speak_now)
                    .setContentTitle("Sassy-Talk")
                    .setContentText("Incoming radio")
                    .build()
            )
            .build()
        try {
            nm.notify(FcmWakePolicy.WAKE_NOTIFICATION_ID, n)
        } catch (t: Throwable) {
            Log.w(TAG, "wake notification failed: ${t.message}")
        }
    }

    /** Best-effort: read the current active session id (room) from native. */
    private fun currentRoomId(): String? {
        return try {
            SassyTalkNative.appContext = applicationContext
            SassyTalkNative.getSessionId()
        } catch (t: Throwable) {
            null
        }
    }
}
