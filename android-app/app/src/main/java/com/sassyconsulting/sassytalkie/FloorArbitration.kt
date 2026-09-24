// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
package com.sassyconsulting.sassytalkie

/**
 * Pure deterministic busy-channel and emergency-preemption policy.
 *
 * Must match core/src/floor.rs (remote_wins / should_block_local) — do not
 * diverge the two implementations in this pass.
 *
 * Floor occupancy is not the 400 ms UI "peer speaking" LED. That LED
 * blinks off during a cellular gap; using it as the TX lock lets a
 * second radio key up while the first stream is still draining.
 */
internal object FloorArbitration {
    /** UI LED only. Never use this as the TX floor lock. */
    const val UI_SPEAKING_MS = 400L

    /** Keep the floor held after PTT_STOP so the jitter buffer can drain. */
    const val DRAIN_HOLD_MS = 300L

    /**
     * Audio-silence stale hold. Must outlast relay jitter (100–500 ms) plus
     * the Live prebuffer, otherwise a gap looks like "channel free".
     */
    const val STALE_HOLD_MS = 1_500L

    fun shouldBlockLocal(floorHeld: Boolean, localEmergency: Boolean): Boolean =
        floorHeld && !localEmergency

    fun remoteWins(
        localEpoch: Long,
        localEmergency: Boolean,
        remoteEpoch: Long,
        remoteEmergency: Boolean,
        localPeerId: String = "",
        remotePeerId: String = "",
    ): Boolean = when {
        remoteEmergency != localEmergency -> remoteEmergency
        remoteEpoch != localEpoch -> remoteEpoch < localEpoch
        localPeerId.isNotEmpty() && remotePeerId.isNotEmpty() -> remotePeerId < localPeerId
        else -> false
    }
}
