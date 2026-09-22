// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-DTOOWM25QCE7
package com.sassyconsulting.sassytalkie.license

import android.content.Context
import android.util.Log
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.sassyconsulting.sassytalkie.BuildConfig
import com.sassyconsulting.sassytalkie.ui.theme.DarkBg
import com.sassyconsulting.sassytalkie.ui.theme.PrimaryBlue
import com.sassyconsulting.sassytalkie.ui.theme.StatusDisconnected
import com.sassyconsulting.sassytalkie.ui.theme.SurfaceBg
import com.sassyconsulting.sassytalkie.ui.theme.Teal
import com.sassyconsulting.sassytalkie.ui.theme.TextGray
import com.sassyconsulting.sassytalkie.ui.theme.TextWhite
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.json.JSONObject
import java.util.concurrent.TimeUnit

/**
 * Direct-flavor entitlement gate: license-key activation against the relay
 * worker's /license endpoints (cloudflare-worker/src/license.js). Same
 * contract as the Play flavor's Entitlements — AppNavigation is flavor-blind.
 *
 * Offline model: activation returns an HMAC-signed receipt with a 30-day
 * expiry. The app treats the receipt's expiry as its offline entitlement
 * horizon and silently re-validates whenever [refresh] runs with network
 * available, sliding the window forward. Revoked/refunded keys therefore die
 * within 30 days; paying users who camp off-grid stay unlocked for the same.
 */
object Entitlements {
    private const val TAG = "Entitlements"

    // Re-validate opportunistically once the receipt is past its half-life,
    // so one successful check per ~15 days keeps a live key permanently warm.
    private const val REFRESH_WHEN_REMAINING_SEC = 15L * 24 * 3600

    // Mirrors the worker's canonical license shape; anything else the user
    // enters is treated as a promo code (worker enforces promo shape).
    private val LICENSE_RE = Regex("^SASSY(-[A-HJ-NP-Z2-9]{5}){4}$")

    private fun endpointFor(credential: String, kind: String?): String = when {
        kind == "promo" -> "/license/promo"
        kind == "license" -> "/license/validate"
        LICENSE_RE.matches(credential) -> "/license/activate"
        else -> "/license/promo"
    }

    private val http = OkHttpClient.Builder()
        .connectTimeout(10, TimeUnit.SECONDS)
        .readTimeout(10, TimeUnit.SECONDS)
        .build()

    fun isUnlockedCached(context: Context): Boolean {
        // Debug builds are always entitled: since the transport gate moved
        // below the UI (AutoConnectManager.autoConnect), a fresh sideloaded
        // debug install with no receipt got ZERO connections — dead radio on
        // every dev device and emulator. Release builds do not take this path
        // (BuildConfig.DEBUG is false in release).
        if (BuildConfig.DEBUG) return true
        val p = LicenseStore.prefs(context) ?: return false
        val exp = p.getLong(LicenseStore.KEY_RECEIPT_EXP, 0L)
        return exp > System.currentTimeMillis() / 1000
    }

    /**
     * Slide the receipt window forward if it's past its half-life. Network
     * errors are ignored (still inside the receipt window); an explicit
     * rejection from the server clears the entitlement.
     */
    fun refresh(context: Context, onResult: (Boolean) -> Unit = {}) {
        // Debug builds are always entitled — must mirror isUnlockedCached's
        // bypass. Without this, AppNavigation's silent reconciliation called
        // refresh() a beat after startup, got `false` (no stored license on a
        // dev install), and flipped an already-unlocked debug session onto
        // the paywall gate. Release builds do not take this path
        // (BuildConfig.DEBUG is false in release).
        if (BuildConfig.DEBUG) return onResult(true)
        val p = LicenseStore.prefs(context) ?: return onResult(false)
        val key = p.getString(LicenseStore.KEY_LICENSE, null) ?: return onResult(false)
        val kind = p.getString(LicenseStore.KEY_KIND, null)
        val exp = p.getLong(LicenseStore.KEY_RECEIPT_EXP, 0L)
        val now = System.currentTimeMillis() / 1000
        if (exp - now > REFRESH_WHEN_REMAINING_SEC) return onResult(true)

        CoroutineScope(Dispatchers.IO).launch {
            when (val res = call(endpointFor(key, kind), key, context)) {
                is ApiResult.Ok -> {
                    p.edit().putLong(LicenseStore.KEY_RECEIPT_EXP, res.expiresAt).apply()
                    onResult(true)
                }
                is ApiResult.Rejected -> {
                    // A "device not activated" rejection on /validate means the
                    // slot row is gone (server migration, deactivated elsewhere)
                    // though the key may still be valid — RE-CLAIM a slot via
                    // /activate before revoking, so a lost row doesn't strand a
                    // paying user. Genuine "revoked"/"maximum devices" rejections
                    // fall through and clear the receipt as before.
                    val reclaimed = if (endpointFor(key, kind) == "/license/validate" &&
                        res.message.contains("not activated", ignoreCase = true)) {
                        call("/license/activate", key, context)
                    } else {
                        null
                    }
                    if (reclaimed is ApiResult.Ok) {
                        p.edit().putLong(LicenseStore.KEY_RECEIPT_EXP, reclaimed.expiresAt).apply()
                        onResult(true)
                    } else {
                        Log.w(TAG, "License rejected on revalidation: ${res.message}")
                        p.edit().remove(LicenseStore.KEY_RECEIPT_EXP).apply()
                        onResult(false)
                    }
                }
                is ApiResult.NetworkError -> onResult(exp > now) // ride out the receipt
            }
        }
    }

    @Composable
    fun GateScreen(onUnlocked: () -> Unit) {
        val context = LocalContext.current
        var keyInput by remember { mutableStateOf("") }
        var error by remember { mutableStateOf<String?>(null) }
        var busy by remember { mutableStateOf(false) }
        val scope = rememberCoroutineScope()

        Box(
            modifier = Modifier.fillMaxSize().background(DarkBg),
            contentAlignment = Alignment.Center,
        ) {
            // Scroll + imePadding: the key/promo field lives low in this
            // column and the soft keyboard covered it — typed text was
            // invisible under the IME. Mirrors the play-flavor gate fix.
            Column(
                horizontalAlignment = Alignment.CenterHorizontally,
                modifier = Modifier
                    .padding(32.dp)
                    .verticalScroll(rememberScrollState())
                    .imePadding(),
            ) {
                Text(text = "🎙", fontSize = 64.sp)
                Spacer(modifier = Modifier.height(24.dp))
                Text(
                    text = "Activate Sassy-Talk",
                    fontSize = 24.sp,
                    fontWeight = FontWeight.Bold,
                    color = Teal,
                )
                Spacer(modifier = Modifier.height(12.dp))
                Text(
                    // Say WHY this screen appeared. Arriving at a bare paywall
                    // after five working sessions reads as the app breaking;
                    // naming the trial makes it a transaction the user
                    // recognises rather than a fault.
                    text = if (TrialGate.trialExhausted(context)) {
                        "You've used your ${TrialGate.FREE_SESSIONS} free sessions.\n" +
                            "Enter your license key from checkout,\nor a friends & family promo code."
                    } else {
                        "Enter your license key from checkout,\nor a friends & family promo code.\nLicenses cover 3 devices."
                    },
                    fontSize = 14.sp,
                    color = TextGray,
                    textAlign = TextAlign.Center,
                )
                Spacer(modifier = Modifier.height(24.dp))
                OutlinedTextField(
                    value = keyInput,
                    onValueChange = { keyInput = it.uppercase() },
                    placeholder = { Text("License key or promo code", color = TextGray) },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Characters),
                    colors = OutlinedTextFieldDefaults.colors(
                        focusedTextColor = TextWhite,
                        unfocusedTextColor = Teal,
                        focusedBorderColor = PrimaryBlue,
                        unfocusedBorderColor = TextGray,
                        cursorColor = Teal,
                        focusedContainerColor = SurfaceBg,
                        unfocusedContainerColor = SurfaceBg,
                    ),
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(modifier = Modifier.height(24.dp))
                if (busy) {
                    CircularProgressIndicator(color = Teal)
                } else {
                    Button(
                        onClick = {
                            busy = true
                            error = null
                            val credential = keyInput.trim()
                            val isLicense = LICENSE_RE.matches(credential)
                            scope.launch {
                                val res = withContext(Dispatchers.IO) {
                                    call(
                                        if (isLicense) "/license/activate" else "/license/promo",
                                        credential,
                                        context,
                                    )
                                }
                                busy = false
                                when (res) {
                                    is ApiResult.Ok -> {
                                        LicenseStore.prefs(context)?.edit()
                                            ?.putString(LicenseStore.KEY_LICENSE, credential)
                                            ?.putString(LicenseStore.KEY_KIND, if (isLicense) "license" else "promo")
                                            ?.putLong(LicenseStore.KEY_RECEIPT_EXP, res.expiresAt)
                                            ?.apply()
                                        onUnlocked()
                                    }
                                    is ApiResult.Rejected -> error = res.message
                                    is ApiResult.NetworkError ->
                                        error = "Can't reach the license server — check your connection"
                                }
                            }
                        },
                        enabled = keyInput.isNotBlank(),
                        colors = ButtonDefaults.buttonColors(containerColor = PrimaryBlue),
                        shape = RoundedCornerShape(25.dp),
                        modifier = Modifier.height(52.dp).width(240.dp),
                    ) {
                        Text("Activate", fontSize = 16.sp)
                    }
                }
                Spacer(modifier = Modifier.height(16.dp))
                Text(
                    text = "Need a key? sassyconsultingllc.com",
                    fontSize = 13.sp,
                    color = TextGray,
                )
                error?.let {
                    Spacer(modifier = Modifier.height(16.dp))
                    Text(
                        text = it,
                        fontSize = 13.sp,
                        color = StatusDisconnected,
                        textAlign = TextAlign.Center,
                    )
                }
            }
        }
    }

    // ── Worker API ────────────────────────────────────────────────────────

    private sealed class ApiResult {
        class Ok(val expiresAt: Long) : ApiResult()
        class Rejected(val message: String) : ApiResult()
        object NetworkError : ApiResult()
    }

    private fun call(path: String, key: String, context: Context): ApiResult {
        return try {
            // "key" feeds /license/activate|validate, "code" feeds /license/promo —
            // sending both lets one helper serve every endpoint.
            val body = JSONObject()
                .put("key", key)
                .put("code", key)
                .put("device_id", deviceId(context))
                .put("device_name", android.os.Build.MODEL)
                .put("app_version", BuildConfig.VERSION_NAME)
                .toString()
                .toRequestBody("application/json".toMediaType())
            val req = Request.Builder()
                .url(BuildConfig.LICENSE_API_BASE + path)
                .post(body)
                .build()
            http.newCall(req).execute().use { resp ->
                val json = JSONObject(resp.body?.string() ?: "{}")
                when {
                    resp.isSuccessful && json.optBoolean("ok") ->
                        ApiResult.Ok(json.optLong("expires_at"))
                    // 5xx = server trouble, treat like network (don't burn the
                    // cached receipt over an outage). 4xx = a real rejection.
                    resp.code >= 500 -> ApiResult.NetworkError
                    else -> ApiResult.Rejected(json.optString("error", "License rejected"))
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "license call failed: ${e.message}")
            ApiResult.NetworkError
        }
    }

    // Stable per-install id (ANDROID_ID, or a persisted random fallback when it
    // is null) — the granularity a device-slot count wants. The server stores
    // only a salted HMAC of it.
    private fun deviceId(context: Context): String = LicenseStore.deviceId(context)
}
