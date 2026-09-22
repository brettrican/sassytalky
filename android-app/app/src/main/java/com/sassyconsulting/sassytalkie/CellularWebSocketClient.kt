// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-32PLBELNC65M
package com.sassyconsulting.sassytalkie

import android.util.Log
import okhttp3.*
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okio.ByteString
import okio.ByteString.Companion.toByteString
import org.json.JSONObject
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger

/**
 * WebSocket client for cellular PTT relay.
 *
 * Architecture:
 *   Kotlin OkHttp WebSocket ↔ Cloudflare Durable Object relay ↔ other devices
 *
 * Data flow:
 *   TX: Rust send_audio() → outbound queue → pollOutbound() loop → WS.send(binary)
 *   RX: WS.onMessage(binary) → cellularOnMessage() → inbound queue → Rust receive_audio()
 *
 * The relay is a blind forwarder — all encryption/decryption happens in Rust.
 */
class CellularWebSocketClient {

    companion object {
        private const val TAG = "CellularWS"
        // Idle poll between JNI outbound drains. Was 2 ms (~500 JNI/s while
        // idle); 20 ms still keeps TX latency fine for 20 ms Opus frames and
        // the pump drains the full queue each wake for bursts.
        private const val POLL_INTERVAL_MS = 20L
        private const val PING_INTERVAL_SEC = 15L
        private const val MAX_RECONNECT_ATTEMPTS = 8
        // After the fast exponential retries, keep trying at this fixed floor
        // instead of giving up, so an outage longer than the first 8 attempts
        // still self-heals once connectivity returns (a successful connect
        // resets the attempt counter).
        private const val RECONNECT_FLOOR_MS = 60_000L
        // The relay force-closes (1008) only after extreme sustained abuse;
        // soft-drops start at ~120 msg/s. Pace under the soft cap so a
        // post-stall burst doesn't lose frames to soft-drop either.
        private const val MAX_SENDS_PER_SEC = 100
        // OkHttp WebSocket.send() returns false when its outbound buffer is
        // full — previously ignored, which silently dropped frames and looked
        // like ~30% relay loss under cellular backpressure.
        //
        // 3×5 ms = 15 ms retry ceiling (was 8×8 ms = 64 ms). Longer ceilings
        // help throughput on a briefly-stalled link but hurt real-time audio:
        // during the block the pump can't advance to newer frames, and 40 ms
        // of stall = 2 dropped Opus frames of listener-side latency. The
        // outbound queue already drop-oldests on overflow (cellular_transport.rs
        // PacketQueue), so giving up faster loses at most one frame per stall
        // instead of piling latency behind a doomed one.
        private const val SEND_RETRY_MAX = 3
        private const val SEND_RETRY_SLEEP_MS = 5L
        // Local counter exposed via getSendDropCount() for diagnostics.
        @JvmStatic
        @Volatile
        var cumulativeSendDrops: Long = 0
            private set
        fun noteSendDrop() { cumulativeSendDrops++ }
        // The relay DO reaps any socket silent for >8s (1001 "Heartbeat stale").
        // Send a keepalive comfortably under that when nothing else is (see
        // sendKeepAlive) so a relay-only device doesn't flap-reconnect endlessly.
        private const val KEEPALIVE_INTERVAL_MS = 4_000L

        /**
         * OkHttpClient is expensive (dispatcher + connection pool + thread pools)
         * and explicitly designed to be shared. Creating one per instance wasted
         * threads and defeated pooling.
         */
        private val sharedClient: OkHttpClient = OkHttpClient.Builder()
            .readTimeout(0, TimeUnit.MILLISECONDS)
            .pingInterval(PING_INTERVAL_SEC, TimeUnit.SECONDS)
            .build()
    }

    private val client: OkHttpClient =
        RelayTlsPins.apply(sharedClient.newBuilder()).build()

    private var webSocket: WebSocket? = null
    private val isConnected = AtomicBoolean(false)
    private val isRunning = AtomicBoolean(false)
    private val isConnecting = AtomicBoolean(false)
    private val reconnectAttempts = AtomicInteger(0)
    /** True after at least one successful WS open on this client — drives ?catchup=1. */
    @Volatile private var hasCompletedHandshake = false
    /** Invalidates auth callbacks, sockets, pumps, and reconnect tasks together. */
    private val generation = GenerationOwner()
    // Set by disconnect(), cleared by an explicit connect(). While true, every
    // reconnect path is inert. This replaces the old "poison the attempt
    // counter" trick, which stopped working when scheduleReconnect gained the
    // never-give-up floor: a torn-down client would resurrect 60s later as a
    // second socket in the room, stealing outbound frames, duplicating inbound
    // ones, and flapping the shared native transport state on every DO reap.
    private val closed = AtomicBoolean(false)
    private var outboundThread: Thread? = null

    // Outbound send-rate window — single-thread state, touched only by the
    // outbound pump (see paceSend).
    private var sendWindowStart = 0L
    private var sendWindowCount = 0

    /**
     * Single-slot scheduler for reconnect attempts. Previously each failure
     * spawned a fresh Thread that slept and then called connect(); bursts of
     * failures could race and open multiple sockets. Using a single scheduler
     * with a cancel-before-schedule pattern ensures at most one reconnect is
     * in flight at any moment.
     */
    private val reconnectScheduler: ScheduledExecutorService =
        Executors.newSingleThreadScheduledExecutor { r ->
            Thread(r, "cellular-reconnect").apply { isDaemon = true }
        }
    @Volatile private var pendingReconnect: ScheduledFuture<*>? = null

    // Relay keepalive — decouples socket liveness from Bluetooth. The real
    // heartbeat lives on PttCoordinator (only created when BT is initialised),
    // so a relay-only device would otherwise go silent and get reaped every 8s.
    private val keepAliveScheduler: ScheduledExecutorService =
        Executors.newSingleThreadScheduledExecutor { r ->
            Thread(r, "cellular-keepalive").apply { isDaemon = true }
        }
    private var keepAliveTask: ScheduledFuture<*>? = null
    private val keepAliveEpoch: Long = System.currentTimeMillis()
    private val keepAliveSeq = AtomicInteger(0)

    /** Callback for DO readiness confirmation. */
    var onRelayReady: (() -> Unit)? = null

    /**
     * Fired once when a previously-open relay socket transitions to down
     * (graceful close, failure, or user disconnect). AutoConnect uses this
     * for sticky reconnect (keep the relay hub through brief blips) instead
     * of immediately failing over to Bluetooth.
     */
    var onRelayLost: ((reason: String) -> Unit)? = null

    /**
     * Stable per-install peer identifier appended to the WS URL as `peer=...`.
     * The relay uses it to map WS sessions to /presence registrations so audio
     * for a peer with no active WS triggers an FCM wake push. Set this from
     * the wire-up site (AutoConnectManager) via InstallId.get(context).
     */
    var peerId: String? = null

    /** PttCoordinator to route binary control frames (opcodes 0x10–0x1F). */
    var pttCoordinator: PttCoordinator? = null

    /** Connect to the cellular relay. Room must be set first via SassyTalkNative.cellularSetRoom() */
    fun connect(): Boolean {
        // An explicit connect() revives a client that was disconnect()ed
        // (WalkieService.forceCellularReconnect does exactly this on room
        // changes). Fresh backoff state too — otherwise one transient auth
        // failure right after the forced reconnect lands on the 60s floor
        // instead of the 3s fast retry, and a share-link joiner sits dead
        // in the old room for a minute.
        if (closed.getAndSet(false)) {
            reconnectAttempts.set(0)
        }
        if (isConnected.get() || !isConnecting.compareAndSet(false, true)) {
            Log.d(TAG, "Already connected/connecting")
            return true
        }
        val ownerGeneration = generation.next()
        // Respect build-time flag to disable cellular relay (opt-in at build)
        try {
            if (!BuildConfig.ENABLE_CELLULAR_RELAY) {
                Log.w(TAG, "Cellular relay disabled by build config")
                isConnecting.set(false)
                return false
            }
        } catch (e: Throwable) {
            // If BuildConfig is not available for any reason, proceed normally
        }

        val baseWsUrl = SassyTalkNative.cellularGetWsUrl()
        if (baseWsUrl.isBlank()) {
            Log.e(TAG, "No WS URL — set room first")
            isConnecting.set(false)
            return false
        }
        // Room id (= QR session_id). Log a short fingerprint so split-room
        // pairing bugs are obvious in logcat without dumping the full UUID.
        val roomFp = runCatching {
            android.net.Uri.parse(baseWsUrl).getQueryParameter("room")?.take(8)
        }.getOrNull() ?: "?"
        Log.i(TAG, "Connecting cellular room=$roomFp")

        // Fetch the capability token asynchronously. Doing this synchronously
        // here would block the calling thread (typically Dispatchers.Main via
        // AutoConnectManager.scope.launch) and throw NetworkOnMainThreadException,
        // which manifested as the cell transport flipping CONNECTED→---→CONNECTED
        // every reconnect cycle. enqueue() runs on the OkHttp dispatcher.
        Log.i(TAG, "Fetching relay auth token…")
        authorizeWsUrlAsync(baseWsUrl) { wsUrl, err ->
            if (!generation.owns(ownerGeneration) || closed.get()) {
                Log.d(TAG, "Ignoring stale auth result generation=$ownerGeneration")
                return@authorizeWsUrlAsync
            }
            if (err != null || wsUrl == null) {
                isConnecting.set(false)
                Log.e(TAG, "Auth fetch failed: ${err?.message}")
                SassyTalkNative.cellularOnError("auth: ${err?.message ?: "unknown"}")
                if (isTerminalAuthError(err)) {
                    closed.set(true)
                    Log.e(TAG, "Terminal relay authentication failure; reconnect disabled until explicit connect")
                } else {
                    scheduleReconnect("auth failure: ${err?.message}")
                }
                return@authorizeWsUrlAsync
            }
            // disconnect() may have raced the auth round-trip — don't open a
            // socket for a client that was torn down while we were fetching.
            openWebSocketAuthenticated(wsUrl, ownerGeneration)
        }

        return true // Connection is async; status comes via onOpen
    }

    private fun openWebSocketAuthenticated(wsUrl: String, ownerGeneration: Int) {
        Log.i(TAG, "Connecting to relay (authenticated)")

        val request = Request.Builder()
            .url(wsUrl)
            .build()

        webSocket = client.newWebSocket(request, object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                if (!generation.owns(ownerGeneration) || closed.get()) {
                    webSocket.close(1000, "stale generation")
                    return
                }
                Log.i(TAG, "WebSocket opened")
                this@CellularWebSocketClient.webSocket = webSocket
                isConnecting.set(false)
                isConnected.set(true)
                hasCompletedHandshake = true
                reconnectAttempts.set(0)
                SassyTalkNative.cellularOnConnected()
                startOutboundPump()
                startKeepAlive()
            }

            override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
                if (!generation.owns(ownerGeneration)) return
                val raw = bytes.toByteArray()
                // Validate full TLV structure before routing to PttCoordinator:
                // byte[0] opcode in 0x10..0x20, bytes[1..2] payload length (u16 LE),
                // total frame size must equal 3 + payloadLen.
                if (raw.size >= 3) {
                    val op = raw[0].toInt() and 0xFF
                    if (op in 0x10..0x20) {
                        val payloadLen = (raw[1].toInt() and 0xFF) or ((raw[2].toInt() and 0xFF) shl 8)
                        if (raw.size == 3 + payloadLen) {
                            // Derive a per-peer routing key from the TLV payload
                            // when possible. Using a constant "relay" key
                            // collapsed every cellular peer into one entry in
                            // PttCoordinator's LivenessTracker, so the Users
                            // tab showed N real peers as a single pseudo-peer
                            // and presence/health was meaningless.
                            val peerId = relayPeerIdFromFrame(raw) ?: "relay"
                            pttCoordinator?.onControlFrame(peerId, raw)
                            return
                        }
                    }
                }
                // Otherwise treat as encrypted audio frame
                com.sassyconsulting.sassytalkie.debug.AudioTelemetry.onPacketReceived(raw.size)
                SassyTalkNative.cellularOnMessage(raw)
            }

            override fun onMessage(webSocket: WebSocket, text: String) {
                if (!generation.owns(ownerGeneration)) return
                if (BuildConfig.DEBUG) Log.d(TAG, "Control: $text")
                try {
                    val obj = JSONObject(text)
                    when (obj.optString("type")) {
                        "welcome" -> {
                            Log.i(TAG, "DO welcome received — relay is ready")
                            onRelayReady?.invoke()
                        }
                        "peer_joined" -> {
                            val device = obj.optString("device", "Peer")
                            val clientId = obj.optString("client_id", "")
                            val peerKey = when {
                                clientId.isNotEmpty() -> "relay:$clientId"
                                device.isNotEmpty() -> "relay:peer:$device"
                                else -> return
                            }
                            pttCoordinator?.onRelayPeerSeen(peerKey, device)
                        }
                        "peer_left" -> {
                            val clientId = obj.optString("client_id", "")
                            if (clientId.isNotEmpty()) {
                                pttCoordinator?.onRelayPeerGone("relay:$clientId")
                            }
                        }
                    }
                } catch (t: Throwable) {
                    Log.w(TAG, "control parse failed: ${t.message}")
                }
            }

            override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                if (!generation.owns(ownerGeneration)) return
                Log.i(TAG, "WebSocket closing: $code $reason")
                webSocket.close(1000, null)
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                if (!generation.owns(ownerGeneration)) return
                Log.i(TAG, "WebSocket closed: $code $reason")
                onDisconnected("closed: $code $reason", ownerGeneration)
                // Server-initiated close (e.g. DO restart) previously left us
                // permanently disconnected. Retry via the same backoff as
                // onFailure so the radio recovers automatically.
                scheduleReconnect("graceful close $code")
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                if (!generation.owns(ownerGeneration)) return
                Log.e(TAG, "WebSocket failure: ${t.message}")
                SassyTalkNative.cellularOnError(t.message ?: "unknown error")
                onDisconnected("failure: ${t.message}", ownerGeneration)
                scheduleReconnect("failure: ${t.message}")
            }
        }).also {
            if (generation.owns(ownerGeneration)) webSocket = it else it.cancel()
        }
    }

    /** Disconnect from the relay */
    fun disconnect() {
        Log.i(TAG, "Disconnecting")
        // User-initiated disconnect must not be overridden by an auto-reconnect
        // — including the one onClosed() fires when the close handshake for
        // THIS disconnect completes a moment from now.
        closed.set(true)
        generation.invalidate()
        isConnecting.set(false)
        cancelPendingReconnect()
        stopOutboundPump()
        webSocket?.close(1000, "user disconnect")
        webSocket = null
        onDisconnected("user disconnect")
    }

    /** Terminal close: stop reconnect and release scheduler threads. */
    fun shutdown() {
        disconnect()
        stopKeepAlive()
        try { reconnectScheduler.shutdownNow() } catch (_: Throwable) {}
        try { keepAliveScheduler.shutdownNow() } catch (_: Throwable) {}
    }

    /**
     * Schedule a single pending reconnect attempt with capped exponential
     * backoff. Cancels any prior pending attempt so we never have more than
     * one reconnect in flight concurrently.
     */
    private fun scheduleReconnect(cause: String) {
        // A disconnect()ed client must stay down until someone explicitly
        // calls connect() on it again. Without this, the graceful-close
        // callback for the disconnect itself re-arms a reconnect and the
        // discarded instance comes back as a zombie socket.
        if (closed.get()) {
            Log.d(TAG, "Reconnect suppressed ($cause): client is closed")
            return
        }
        val attempt = reconnectAttempts.incrementAndGet()
        // Don't give up after the fast retries — fall back to a fixed long
        // interval so a prolonged outage still recovers on its own once
        // connectivity returns (a successful connect resets the counter).
        val delayMs = if (attempt <= MAX_RECONNECT_ATTEMPTS) {
            minOf(3_000L * (1L shl (attempt - 1).coerceAtMost(4)), 60_000L)
        } else {
            RECONNECT_FLOOR_MS
        }
        val scheduledGeneration = generation.current()
        cancelPendingReconnect()
        pendingReconnect = reconnectScheduler.schedule({
            if (generation.owns(scheduledGeneration) && !isConnected.get() && !closed.get()) {
                Log.i(TAG, "Reconnecting ($cause, attempt $attempt, delay ${delayMs}ms)…")
                try { connect() } catch (e: Exception) {
                    Log.w(TAG, "Reconnect attempt $attempt threw: ${e.message}")
                }
            }
        }, delayMs, TimeUnit.MILLISECONDS)
    }

    private fun cancelPendingReconnect() {
        pendingReconnect?.cancel(false)
        pendingReconnect = null
    }

    fun isAttempting(): Boolean =
        !closed.get() && (isConnected.get() || isConnecting.get() || pendingReconnect?.isDone == false)

    fun isConnected(): Boolean = isConnected.get()

    // ── Outbound pump: polls Rust queue and sends via WebSocket ──

    /**
     * Throttle outbound sends to [MAX_SENDS_PER_SEC]. Normal 50 fps audio never
     * hits it; a post-stall burst (many queued frames flushing at once) would
     * otherwise exceed the relay's ~120 msg/s cap and get the socket 1008-closed.
     * Runs on the outbound thread, so a short Thread.sleep here is fine.
     */
    private fun paceSend() {
        val now = System.currentTimeMillis()
        if (now - sendWindowStart >= 1000L) {
            sendWindowStart = now
            sendWindowCount = 0
        }
        sendWindowCount++
        if (sendWindowCount > MAX_SENDS_PER_SEC) {
            val remaining = 1000L - (System.currentTimeMillis() - sendWindowStart)
            if (remaining > 0) {
                try { Thread.sleep(remaining) } catch (e: InterruptedException) {
                    Thread.currentThread().interrupt()
                }
            }
            sendWindowStart = System.currentTimeMillis()
            sendWindowCount = 1
        }
    }

    /**
     * Send [packet] via the WebSocket, retrying briefly when OkHttp's outbound
     * buffer is full (`send` returns false). Without this, frames were
     * silently discarded under cellular backpressure and surfaced as high
     * "relay packet loss" in diagnostics.
     */
    private fun sendWithRetry(ws: WebSocket, packet: ByteArray): Boolean {
        val payload = packet.toByteString()
        var attempt = 0
        while (attempt < SEND_RETRY_MAX && isConnected.get()) {
            val accepted = try {
                ws.send(payload)
            } catch (e: Exception) {
                Log.w(TAG, "WS send threw: ${e.message}")
                false
            }
            if (accepted) return true
            attempt++
            try {
                Thread.sleep(SEND_RETRY_SLEEP_MS)
            } catch (e: InterruptedException) {
                Thread.currentThread().interrupt()
                return false
            }
        }
        noteSendDrop()
        Log.w(TAG, "WS send dropped after $SEND_RETRY_MAX retries (${packet.size}B)")
        return false
    }

    private fun startOutboundPump() {
        if (isRunning.getAndSet(true)) return

        outboundThread = Thread({
            Log.i(TAG, "Outbound pump started")
            while (isRunning.get() && isConnected.get()) {
                try {
                    // Drain ALL queued packets per wake — bursts (e.g. from
                    // PTT release buffer-mode, or post-jitter catch-up) go
                    // out in one tight loop without paying the poll
                    // interval per frame. Only after the queue is empty do
                    // we sleep, and even then only briefly.
                    var sentAny = false
                    while (isRunning.get() && isConnected.get()) {
                        val packet = SassyTalkNative.cellularPollOutbound() ?: break
                        if (packet.isEmpty()) break
                        paceSend()
                        val ws = webSocket ?: break
                        sendWithRetry(ws, packet)
                        sentAny = true
                    }
                    if (!sentAny) {
                        Thread.sleep(POLL_INTERVAL_MS)
                    }
                } catch (e: InterruptedException) {
                    break
                } catch (e: Exception) {
                    Log.e(TAG, "Outbound pump error: ${e.message}")
                    Thread.sleep(50)
                }
            }
            Log.i(TAG, "Outbound pump stopped")
        }, "cellular-outbound")
        outboundThread?.isDaemon = true
        outboundThread?.start()
    }

    private fun stopOutboundPump() {
        isRunning.set(false)
        outboundThread?.interrupt()
        outboundThread = null
    }

    private fun onDisconnected(reason: String, ownerGeneration: Int? = null) {
        if (ownerGeneration != null && !generation.owns(ownerGeneration)) return
        isConnecting.set(false)
        stopKeepAlive()
        if (isConnected.getAndSet(false)) {
            stopOutboundPump()
            SassyTalkNative.cellularOnDisconnected(reason)
            try {
                onRelayLost?.invoke(reason)
            } catch (t: Throwable) {
                Log.w(TAG, "onRelayLost callback threw: ${t.message}")
            }
        }
        pttCoordinator?.onTransportAvailabilityChanged("relay-$reason")
    }

    /** Send an authenticated binary control frame via the WebSocket relay. */
    fun sendBinary(bytes: ByteArray): Boolean {
        val ws = webSocket ?: return false
        val protected = if (bytes.firstOrNull() == ControlFrame.OP_AUTHENTICATED) {
            bytes
        } else {
            AuthenticatedControlPlane.seal(bytes) ?: run {
                Log.e(TAG, "Control send blocked: no authenticated room context")
                return false
            }
        }
        // Control frames (HB / PTT markers) are small and infrequent — still
        // honour backpressure so we don't silently drop a wake/PTT_START.
        val sent = sendWithRetry(ws, protected)
        if (!sent && BuildConfig.DEBUG) {
            Log.w(TAG, "sendBinary dropped ${protected.size}B control frame")
        }
        return sent
    }

    /** Send a heartbeat ping to the relay (JSON control message) */
    fun sendPing() {
        webSocket?.send("""{"type":"ping"}""")
    }

    // ── Keepalive: keep the relay socket alive independent of Bluetooth ──

    private fun startKeepAlive() {
        stopKeepAlive()
        keepAliveTask = keepAliveScheduler.scheduleWithFixedDelay(
            {
                try { sendKeepAlive() } catch (e: Exception) {
                    Log.w(TAG, "keepalive send failed: ${e.message}")
                }
            },
            KEEPALIVE_INTERVAL_MS, KEEPALIVE_INTERVAL_MS, TimeUnit.MILLISECONDS,
        )
    }

    private fun stopKeepAlive() {
        keepAliveTask?.cancel(false)
        keepAliveTask = null
    }

    /**
     * The relay Durable Object closes any socket that goes >8s without a message
     * ("Heartbeat stale", 1001). The app's real heartbeat lives on PttCoordinator,
     * which only exists once Bluetooth is initialised — so a relay-only device
     * (BT off, no permission, or no BT peers) never heartbeats and gets reaped +
     * reconnected every ~8s. When no PttCoordinator loop is covering liveness,
     * send a heartbeat ourselves so the socket stays up. It MUST be a binary
     * control frame (OP_HEARTBEAT) — that's what refreshes the DO's liveness; a
     * text ping would not, and any binary frame is safely routed to control (not
     * decoded as audio) by receiving peers.
     */
    private fun sendKeepAlive() {
        if (!isConnected.get()) return
        // PttCoordinator's own 2s heartbeat already keeps the socket alive —
        // but only when the coordinator is actually pumping heartbeats through
        // THIS client. A bare `pttCoordinator != null` check was wrong: after
        // tearDownCellularClient() re-wires coord.cellularClient to a new
        // instance (or the coordinator is shut down), the stale back-reference
        // kept suppressing our keepalive while no heartbeats flowed on this
        // socket, and the DO reaped it every 8s ("Heartbeat stale" flap).
        // Checked each fire because the wiring can change at any time.
        if (pttCoordinator?.cellularClient === this) return
        val frame = ControlFrame.encodeHeartbeat(
            keepAliveEpoch,
            keepAliveSeq.getAndIncrement(),
            System.currentTimeMillis(),
            PresenceState.IDLE,
            0,
            SassyTalkNative.localCapabilities(),
        )
        sendBinary(frame)
    }

    /**
     * Fetch an HMAC-signed capability token from the relay's /auth endpoint
     * (async, off the calling thread), then invoke [onResult] with the WS URL
     * carrying the appended token. On failure, [onResult] is called with
     * (null, exception).
     *
     * MUST be async — the caller (CellularWebSocketClient.connect) is invoked
     * from the AutoConnectManager coroutine on Dispatchers.Main, so a blocking
     * execute() throws NetworkOnMainThreadException.
     */
    private fun authorizeWsUrlAsync(baseWsUrl: String, onResult: (String?, Throwable?) -> Unit) {
        val httpUrl = toHttpScheme(baseWsUrl).toHttpUrlOrNull()
        if (httpUrl == null) {
            onResult(null, IllegalStateException("Invalid WS URL: $baseWsUrl"))
            return
        }
        val room = httpUrl.queryParameter("room")
        if (room.isNullOrBlank()) {
            onResult(null, IllegalStateException("WS URL missing room param"))
            return
        }

        val authUrl = httpUrl.newBuilder()
            .encodedPath("/auth")
            .build()
            .newBuilder()
            .setQueryParameter("room", room)
            .apply {
                peerId?.takeIf { it.isNotBlank() }?.let { setQueryParameter("peer", it) }
            }
            .build()

        val req = Request.Builder().url(authUrl).get().build()
        client.newCall(req).enqueue(object : Callback {
            override fun onFailure(call: Call, e: java.io.IOException) {
                onResult(null, e)
            }

            override fun onResponse(call: Call, response: Response) {
                response.use { resp ->
                    try {
                        if (!resp.isSuccessful) {
                            onResult(null, IllegalStateException("auth http ${resp.code}"))
                            return
                        }
                        val bodyText = resp.body?.string()
                        if (bodyText.isNullOrEmpty()) {
                            onResult(null, IllegalStateException("auth empty body"))
                            return
                        }
                        val token = JSONObject(bodyText).optString("token")
                        if (token.isBlank()) {
                            onResult(null, IllegalStateException("auth token missing"))
                            return
                        }
                        val builder = toHttpScheme(baseWsUrl).toHttpUrlOrNull()!!
                            .newBuilder()
                            .setQueryParameter("token", token)
                        // Stable peer-id so the relay can match this WS to a
                        // /presence row and skip FCM pushes when we're online.
                        peerId?.takeIf { it.isNotBlank() }?.let {
                            builder.setQueryParameter("peer", it)
                        }
                        // Reconnect (not the first open of this client): ask the
                        // DO to replay the retained catchup window. No since=
                        // cursor — we don't persist one yet; catchup=1 alone is enough.
                        if (hasCompletedHandshake) {
                            builder.setQueryParameter("catchup", "1")
                        }
                        val authedUrl = builder.build().toString().let { toWsScheme(it) }
                        onResult(authedUrl, null)
                    } catch (t: Throwable) {
                        onResult(null, t)
                    }
                }
            }
        })
    }

    /** OkHttp's HttpUrl parser rejects ws://, so swap to http:// for parsing only. */
    private fun toHttpScheme(url: String): String = when {
        url.startsWith("wss://") -> "https://" + url.removePrefix("wss://")
        url.startsWith("ws://")  -> "http://"  + url.removePrefix("ws://")
        else -> url
    }

    private fun toWsScheme(url: String): String = when {
        url.startsWith("https://") -> "wss://" + url.removePrefix("https://")
        url.startsWith("http://")  -> "ws://"  + url.removePrefix("http://")
        else -> url
    }

    private fun isTerminalAuthError(error: Throwable?): Boolean {
        val message = error?.message ?: return false
        return message.contains("auth http 400") ||
            message.contains("auth http 401") ||
            message.contains("auth http 403")
    }

    /**
     * Extract a stable per-peer routing key from a TLV control frame. The
     * relay broadcasts frames from N peers down a single WebSocket, so we
     * can't use a per-connection identity; we have to look inside the
     * frame. Five of the seven epoch-bearing opcodes (HEARTBEAT, RECV_ACK,
     * EOT_ACK, PTT_START_V2, PTT_STOP_V2) start their payload with the
     * sender's `epoch:i64` (little-endian) — a 64-bit random session id
     * generated once per app start. That's unique enough to use as a
     * LivenessTracker key.
     *
     * Returns "relay:<epoch>" for the five epoch-prefixed ops, or null for
     * frames whose payload does NOT start with epoch (CAPABILITIES, which
     * uses JSON; PARTNER_OFFLINE, which starts with a length byte). Callers
     * fall back to the legacy constant "relay" key in those cases — the
     * tracker entry is just slightly less precise for two infrequent ops.
     */
    private fun relayPeerIdFromFrame(raw: ByteArray): String? {
        if (raw.isEmpty()) return null
        val op = raw[0].toInt() and 0xFF

        // PARTNER_OFFLINE is the only frame that names the peer that left.
        // Layout: [op][len:u16 LE][peer_id_len:u8][peer_id bytes...]
        // Route it under the leaving peer's own per-epoch id is not possible
        // (we only get the server-assigned UUID here), but we CAN at least
        // distinguish offline events for different peers by using their
        // name as the routing key so liveness.removePeer targets correctly.
        if (op == 0x14) {
            if (raw.size < 4) return null
            val idLen = raw[3].toInt() and 0xFF
            if (raw.size < 4 + idLen || idLen == 0) return null
            val name = String(raw, 4, idLen, Charsets.UTF_8)
            return "relay:peer:$name"
        }

        // Epoch-prefixed opcodes: payload starts with i64 LE epoch of the
        // SENDER. Use that as a stable per-peer routing key over the WS.
        // OP_WAKE (0x17) was added in Phase-1 wake-beacon; if it's omitted
        // here every cellular WAKE collapses to the constant 'relay' key
        // and the wake's per-peer epoch tracking goes blind.
        if (raw.size < 3 + 8) return null
        if (op !in setOf(0x10, 0x11, 0x12, 0x15, 0x16, 0x17)) return null
        val bb = java.nio.ByteBuffer.wrap(raw, 3, 8).order(java.nio.ByteOrder.LITTLE_ENDIAN)
        val epoch = bb.long
        // SessionEpoch.generate() never produces 0; treat 0 as "absent" and
        // fall through to the constant key rather than collapse onto a
        // confusing pseudo-id.
        if (epoch == 0L) return null
        return "relay:$epoch"
    }
}
