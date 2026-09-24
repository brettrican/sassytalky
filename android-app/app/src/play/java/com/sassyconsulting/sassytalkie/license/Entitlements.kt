// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-FKJI2JW3AVPW
package com.sassyconsulting.sassytalkie.license

import android.app.Activity
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
import androidx.compose.material3.TextButton
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import kotlin.coroutines.resume
import com.android.billingclient.api.AcknowledgePurchaseParams
import com.android.billingclient.api.BillingClient
import com.android.billingclient.api.BillingClientStateListener
import com.android.billingclient.api.BillingFlowParams
import com.android.billingclient.api.BillingResult
import com.android.billingclient.api.PendingPurchasesParams
import com.android.billingclient.api.ProductDetails
import com.android.billingclient.api.Purchase
import com.android.billingclient.api.QueryProductDetailsParams
import com.android.billingclient.api.QueryPurchasesParams
import com.sassyconsulting.sassytalkie.BuildConfig
import com.sassyconsulting.sassytalkie.ui.theme.DarkBg
import com.sassyconsulting.sassytalkie.ui.theme.PrimaryBlue
import com.sassyconsulting.sassytalkie.ui.theme.StatusDisconnected
import com.sassyconsulting.sassytalkie.ui.theme.SurfaceBg
import com.sassyconsulting.sassytalkie.ui.theme.Teal
import com.sassyconsulting.sassytalkie.ui.theme.TextGray
import com.sassyconsulting.sassytalkie.ui.theme.TextWhite

/**
 * Play-flavor entitlement gate: Google Play Billing purchase, with promo-code
 * redemption for friends & family (relay `/license/promo`).
 *
 * Contract shared with the direct flavor (same fully-qualified name, different
 * source set — AppNavigation compiles against whichever flavor is built):
 *   - [isUnlockedCached]  fast, offline check for startup routing
 *   - [refresh]           silent background re-verification (restores the
 *                         purchase on reinstall, drops it after a refund)
 *   - [GateScreen]        full-screen paywall shown while locked
 */
object Entitlements {
    private const val TAG = "Entitlements"
    private const val PRODUCT_ID = "sassytalkie_unlock"

    fun isUnlockedCached(context: Context): Boolean {
        // Debug builds are always entitled: since the transport gate moved
        // below the UI (AutoConnectManager.autoConnect), a fresh sideloaded
        // debug install with no purchase got ZERO connections — dead radio on
        // every dev device and emulator. Release builds are unaffected.
        if (BuildConfig.DEBUG) return true
        val p = LicenseStore.prefs(context) ?: return false
        if (p.getBoolean(LicenseStore.KEY_UNLOCKED, false)) return true
        return LicensePromo.hasValidReceipt(context)
    }

    /**
     * Reconcile entitlement: promo receipts refresh against the relay worker;
     * otherwise reconnect to Play and reconcile the cached purchase.
     */
    fun refresh(context: Context, onResult: (Boolean) -> Unit = {}) {
        val appContext = context.applicationContext
        // Debug builds are always entitled — must mirror isUnlockedCached's
        // bypass, or the post-startup reconciliation re-locks a dev install
        // onto the paywall (no Play purchase on emulators).
        if (BuildConfig.DEBUG) return onResult(true)
        if (LicenseStore.prefs(appContext)?.getString(LicenseStore.KEY_KIND, null) == "promo") {
            if (!LicensePromo.hasValidReceipt(appContext)) {
                onResult(false)
                return
            }
            LicensePromo.refreshIfNeeded(appContext) { ok ->
                onResult(ok || LicensePromo.hasValidReceipt(appContext))
            }
            return
        }
        // Guarantee onResult fires EXACTLY once. Play Billing may drop the
        // connection (onBillingServiceDisconnected) or never call back at all;
        // a missed callback would hang the Restore button's coroutine forever.
        val done = java.util.concurrent.atomic.AtomicBoolean(false)
        val client = BillingClient.newBuilder(appContext)
            .setListener { _, _ -> } // refresh never launches a flow
            .enablePendingPurchases(
                PendingPurchasesParams.newBuilder().enableOneTimeProducts().build(),
            )
            .enableAutoServiceReconnection()
            .build()
        val main = android.os.Handler(android.os.Looper.getMainLooper())
        fun finish(result: Boolean) {
            if (!done.compareAndSet(false, true)) return
            main.removeCallbacksAndMessages(null) // cancel the timeout
            try { client.endConnection() } catch (_: Exception) {}
            onResult(result)
        }
        // Hard timeout: if Play never responds, fall back to the cached state
        // rather than leaving the caller hanging.
        main.postDelayed({ finish(isUnlockedCached(appContext)) }, 12_000L)
        client.startConnection(object : BillingClientStateListener {
            override fun onBillingSetupFinished(result: BillingResult) {
                if (result.responseCode != BillingClient.BillingResponseCode.OK) {
                    finish(isUnlockedCached(appContext))
                    return
                }
                client.queryPurchasesAsync(
                    QueryPurchasesParams.newBuilder()
                        .setProductType(BillingClient.ProductType.INAPP).build(),
                ) { qr, purchases ->
                    if (qr.responseCode != BillingClient.BillingResponseCode.OK) {
                        finish(isUnlockedCached(appContext))
                        return@queryPurchasesAsync
                    }
                    val owned = purchases.any {
                        it.products.contains(PRODUCT_ID) &&
                            it.purchaseState == Purchase.PurchaseState.PURCHASED
                    }
                    if (owned) {
                        purchases.filter { it.products.contains(PRODUCT_ID) && !it.isAcknowledged }
                            .forEach { acknowledge(client, it) }
                        setUnlocked(appContext, true)
                        clearUnownedSince(appContext)
                        finish(true)
                    } else {
                        // Do NOT revoke on a single unowned result — Play can
                        // transiently return an empty list. Only drop entitlement
                        // after the purchase stays unowned past a grace window, so
                        // a real refund eventually revokes without a transient
                        // glitch stranding a paying user.
                        val revokedNow = noteUnownedAndMaybeRevoke(appContext)
                        finish(if (revokedNow) false else isUnlockedCached(appContext))
                    }
                }
            }

            override fun onBillingServiceDisconnected() {
                // Without handling this the callback contract is violated and the
                // caller hangs. Fall back to cached state; the timeout backstops
                // the "never called back at all" case.
                finish(isUnlockedCached(appContext))
            }
        })
    }

    @Composable
    fun GateScreen(onUnlocked: () -> Unit) {
        val context = LocalContext.current
        val appContext = context.applicationContext
        val scope = rememberCoroutineScope()
        var price by remember { mutableStateOf<String?>(null) }
        var details by remember { mutableStateOf<ProductDetails?>(null) }
        var error by remember { mutableStateOf<String?>(null) }
        var busy by remember { mutableStateOf(false) }
        var promoInput by remember { mutableStateOf("") }
        var promoBusy by remember { mutableStateOf(false) }
        var catalogLoading by remember { mutableStateOf(true) }
        var catalogAttempt by remember { mutableIntStateOf(0) }

        // One BillingClient per gate visit; listener handles the purchase result.
        val clientHolder = remember { arrayOfNulls<BillingClient>(1) }
        val client = remember {
            BillingClient.newBuilder(appContext)
                .setListener { result, purchases ->
                    val billingClient = clientHolder[0] ?: return@setListener
                    scope.launch(Dispatchers.Main.immediate) {
                        when (result.responseCode) {
                            BillingClient.BillingResponseCode.OK -> {
                                val p = purchases?.firstOrNull {
                                    it.products.contains(PRODUCT_ID) &&
                                        it.purchaseState == Purchase.PurchaseState.PURCHASED
                                }
                                if (p != null) {
                                    acknowledge(billingClient, p)
                                    setUnlocked(appContext, true)
                                    busy = false
                                    onUnlocked()
                                } else {
                                    busy = false
                                }
                            }
                            BillingClient.BillingResponseCode.USER_CANCELED -> busy = false
                            BillingClient.BillingResponseCode.ITEM_ALREADY_OWNED -> {
                                busy = false
                                refresh(appContext) { unlocked ->
                                    scope.launch(Dispatchers.Main.immediate) {
                                        if (unlocked) onUnlocked()
                                        else error = "Purchase exists but could not be restored"
                                    }
                                }
                            }
                            else -> {
                                busy = false
                                error = "Purchase failed (${result.debugMessage})"
                            }
                        }
                    }
                }
                .enablePendingPurchases(
                    PendingPurchasesParams.newBuilder().enableOneTimeProducts().build(),
                )
                .enableAutoServiceReconnection()
                .build()
                .also { clientHolder[0] = it }
        }

        fun applyCatalogResult(
            product: ProductDetails?,
            err: String?,
        ) {
            catalogLoading = false
            details = product
            price = product?.oneTimePurchaseOfferDetailsList
                ?.firstOrNull()
                ?.formattedPrice
                ?: product?.oneTimePurchaseOfferDetails?.formattedPrice
            if (product == null && err != null) error = err
        }

        LaunchedEffect(catalogAttempt) {
            if (LicensePromo.hasValidReceipt(appContext)) {
                // Promo entitlement is receipt-driven (time-limited); do NOT set
                // the permanent KEY_UNLOCKED flag or the promo would never expire.
                onUnlocked()
                return@LaunchedEffect
            }
            catalogLoading = true
            error = null
            details = null
            price = null
            client.endConnection()

            val timedOut = withTimeoutOrNull(15_000L) {
                suspendCancellableCoroutine { cont ->
                    client.startConnection(object : BillingClientStateListener {
                        override fun onBillingSetupFinished(result: BillingResult) {
                            if (result.responseCode != BillingClient.BillingResponseCode.OK) {
                                scope.launch(Dispatchers.Main.immediate) {
                                    applyCatalogResult(
                                        null,
                                        "Google Play unavailable (${result.debugMessage})",
                                    )
                                }
                                if (cont.isActive) cont.resume(Unit)
                                return
                            }
                            // Restore path: existing purchase skips the paywall.
                            client.queryPurchasesAsync(
                                QueryPurchasesParams.newBuilder()
                                    .setProductType(BillingClient.ProductType.INAPP).build(),
                            ) { qr, purchases ->
                                val owned = qr.responseCode == BillingClient.BillingResponseCode.OK &&
                                    purchases.any {
                                        it.products.contains(PRODUCT_ID) &&
                                            it.purchaseState == Purchase.PurchaseState.PURCHASED
                                    }
                                if (owned) {
                                    purchases.filter { !it.isAcknowledged }
                                        .forEach { acknowledge(client, it) }
                                    scope.launch(Dispatchers.Main.immediate) {
                                        catalogLoading = false
                                        setUnlocked(appContext, true)
                                        onUnlocked()
                                    }
                                    if (cont.isActive) cont.resume(Unit)
                                    return@queryPurchasesAsync
                                }
                                val params = QueryProductDetailsParams.newBuilder()
                                    .setProductList(
                                        listOf(
                                            QueryProductDetailsParams.Product.newBuilder()
                                                .setProductId(PRODUCT_ID)
                                                .setProductType(BillingClient.ProductType.INAPP)
                                                .build(),
                                        ),
                                    ).build()
                                client.queryProductDetailsAsync(params) { pr, detailsResult ->
                                    val list = detailsResult.productDetailsList.orEmpty()
                                    scope.launch(Dispatchers.Main.immediate) {
                                        when {
                                            pr.responseCode != BillingClient.BillingResponseCode.OK ->
                                                applyCatalogResult(
                                                    null,
                                                    "Could not load price (${pr.debugMessage})",
                                                )
                                            list.isEmpty() ->
                                                applyCatalogResult(
                                                    null,
                                                    "Unlock product not found in Play Store. " +
                                                        "Confirm \"$PRODUCT_ID\" is published, or tap Restore.",
                                                )
                                            else -> applyCatalogResult(list.first(), null)
                                        }
                                    }
                                    if (cont.isActive) cont.resume(Unit)
                                }
                            }
                        }

                        override fun onBillingServiceDisconnected() {
                            if (cont.isActive) {
                                scope.launch(Dispatchers.Main.immediate) {
                                    applyCatalogResult(
                                        null,
                                        "Lost connection to Google Play — tap Retry",
                                    )
                                }
                                cont.resume(Unit)
                            }
                        }
                    })
                }
            }
            if (timedOut == null && catalogLoading) {
                applyCatalogResult(
                    null,
                    "Google Play is taking too long. Check your connection and tap Retry.",
                )
            }
        }
        DisposableEffect(Unit) { onDispose { client.endConnection() } }

        Box(
            modifier = Modifier.fillMaxSize().background(DarkBg),
            contentAlignment = Alignment.Center,
        ) {
            // Scroll + imePadding: the promo field sits at the BOTTOM of this
            // column, and without these the soft keyboard covered it — the
            // user typed a code they could not see. Scroll keeps every control
            // reachable in the reduced viewport; imePadding lifts the focused
            // field above the IME.
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
                    // Name the trial rather than showing a bare wall — after
                    // five working sessions an unexplained paywall reads as the
                    // app breaking.
                    text = if (TrialGate.trialExhausted(context)) {
                        "Free trial used"
                    } else {
                        "Unlock Sassy-Talk"
                    },
                    fontSize = 24.sp,
                    fontWeight = FontWeight.Bold,
                    color = Teal,
                )
                Spacer(modifier = Modifier.height(12.dp))
                Text(
                    text = "One-time purchase. Encrypted push-to-talk\nover WiFi, Bluetooth, and relay — forever.",
                    fontSize = 14.sp,
                    color = TextGray,
                    textAlign = TextAlign.Center,
                )
                Spacer(modifier = Modifier.height(32.dp))
                when {
                    busy || catalogLoading -> {
                        CircularProgressIndicator(color = Teal)
                        Spacer(modifier = Modifier.height(12.dp))
                        Text(
                            text = if (busy) "Processing purchase…" else "Connecting to Google Play…",
                            fontSize = 13.sp,
                            color = TextGray,
                        )
                    }
                    details != null -> {
                        Button(
                            onClick = {
                                val activity = context as? Activity ?: return@Button
                                val pd = details ?: return@Button
                                busy = true
                                error = null
                                val flowParams = BillingFlowParams.newBuilder()
                                    .setProductDetailsParamsList(
                                        listOf(
                                            BillingFlowParams.ProductDetailsParams.newBuilder()
                                                .setProductDetails(pd).build(),
                                        ),
                                    ).build()
                                val launch = client.launchBillingFlow(activity, flowParams)
                                if (launch.responseCode != BillingClient.BillingResponseCode.OK) {
                                    busy = false
                                    error = "Could not start purchase (${launch.debugMessage})"
                                }
                            },
                            colors = ButtonDefaults.buttonColors(containerColor = PrimaryBlue),
                            shape = RoundedCornerShape(25.dp),
                            modifier = Modifier.height(52.dp).width(240.dp),
                        ) {
                            Text(
                                text = price?.let { "Unlock — $it" } ?: "Unlock",
                                fontSize = 16.sp,
                            )
                        }
                    }
                    else -> {
                        Button(
                            onClick = { catalogAttempt++ },
                            colors = ButtonDefaults.buttonColors(containerColor = PrimaryBlue),
                            shape = RoundedCornerShape(25.dp),
                            modifier = Modifier.height(52.dp).width(240.dp),
                        ) {
                            Text("Retry", fontSize = 16.sp)
                        }
                    }
                }
                if (!catalogLoading && !busy) {
                    Spacer(modifier = Modifier.height(8.dp))
                    TextButton(
                        onClick = {
                            error = null
                            busy = true
                            refresh(appContext) { unlocked ->
                                scope.launch(Dispatchers.Main.immediate) {
                                    busy = false
                                    if (unlocked) onUnlocked()
                                    else error = "No purchase found for this Google account"
                                }
                            }
                        },
                    ) {
                        Text("Already purchased? Restore", color = TextGray, fontSize = 13.sp)
                    }
                }
                if (!busy) {
                    Spacer(modifier = Modifier.height(24.dp))
                    Text(
                        text = "Have a promo code?",
                        fontSize = 13.sp,
                        color = TextGray,
                    )
                    Spacer(modifier = Modifier.height(8.dp))
                    OutlinedTextField(
                        value = promoInput,
                        onValueChange = { promoInput = it.uppercase() },
                        placeholder = { Text("Promo code", color = TextGray) },
                        singleLine = true,
                        enabled = !promoBusy,
                        keyboardOptions = KeyboardOptions(
                            capitalization = KeyboardCapitalization.Characters,
                        ),
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
                    Spacer(modifier = Modifier.height(12.dp))
                    if (promoBusy) {
                        CircularProgressIndicator(color = Teal)
                    } else {
                        Button(
                            onClick = {
                                promoBusy = true
                                error = null
                                val code = promoInput.trim()
                                scope.launch {
                                    val res = withContext(Dispatchers.IO) {
                                        LicensePromo.redeemBlocking(appContext, code)
                                    }
                                    promoBusy = false
                                    when (res) {
                                        LicensePromo.RedeemResult.Ok -> {
                                            // redeemBlocking persisted the time-limited receipt;
                                            // isUnlockedCached honors it via hasValidReceipt. Do NOT
                                            // set the permanent KEY_UNLOCKED flag (a 30-day promo
                                            // would otherwise become permanent).
                                            onUnlocked()
                                        }
                                        LicensePromo.RedeemResult.InvalidFormat ->
                                            error = "Enter a valid promo code (6–40 characters)"
                                        LicensePromo.RedeemResult.NetworkError ->
                                            error = "Can't reach the license server — check your connection"
                                        is LicensePromo.RedeemResult.Rejected ->
                                            error = res.message
                                    }
                                }
                            },
                            enabled = promoInput.isNotBlank(),
                            colors = ButtonDefaults.buttonColors(containerColor = PrimaryBlue),
                            shape = RoundedCornerShape(25.dp),
                            modifier = Modifier.height(48.dp).width(240.dp),
                        ) {
                            Text("Redeem promo", fontSize = 15.sp)
                        }
                    }
                }
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

    private fun setUnlocked(context: Context, value: Boolean) {
        LicenseStore.prefs(context)?.edit()?.putBoolean(LicenseStore.KEY_UNLOCKED, value)?.apply()
    }

    // A refund shows up as Play reporting the one-time product unowned. Because
    // an empty queryPurchasesAsync result is also a common TRANSIENT state, we
    // wait this long (continuously unowned) before dropping the cached unlock.
    private const val REVOKE_GRACE_SEC = 3L * 24 * 3600

    private fun clearUnownedSince(context: Context) {
        LicenseStore.prefs(context)?.edit()?.remove(LicenseStore.KEY_UNOWNED_SINCE)?.apply()
    }

    /**
     * Debounced refund handling: record the first refresh that saw the purchase
     * unowned and revoke only after [REVOKE_GRACE_SEC] of *continuous* unowned,
     * so a transient empty Play query never strands a paying user. Returns true
     * iff it revoked on this call.
     */
    private fun noteUnownedAndMaybeRevoke(context: Context): Boolean {
        val p = LicenseStore.prefs(context) ?: return false
        if (!p.getBoolean(LicenseStore.KEY_UNLOCKED, false)) return false // nothing to revoke
        val now = System.currentTimeMillis() / 1000
        val since = p.getLong(LicenseStore.KEY_UNOWNED_SINCE, 0L)
        if (since == 0L) {
            p.edit().putLong(LicenseStore.KEY_UNOWNED_SINCE, now).apply()
            return false
        }
        if (now - since >= REVOKE_GRACE_SEC) {
            p.edit()
                .putBoolean(LicenseStore.KEY_UNLOCKED, false)
                .remove(LicenseStore.KEY_UNOWNED_SINCE)
                .apply()
            Log.w(TAG, "Play reported no purchase for the grace window — revoking cached unlock")
            return true
        }
        return false
    }

    private fun acknowledge(client: BillingClient, purchase: Purchase) {
        client.acknowledgePurchase(
            AcknowledgePurchaseParams.newBuilder()
                .setPurchaseToken(purchase.purchaseToken).build(),
        ) { r ->
            if (r.responseCode != BillingClient.BillingResponseCode.OK) {
                Log.w(TAG, "acknowledge failed: ${r.debugMessage}")
            }
        }
    }
}
