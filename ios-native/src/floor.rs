// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-IOSFLR2QW8VD
//! floor — runtime floor-occupancy state for iOS.
//!
//! The *policy* (who wins a simultaneous key-up, how long holds last) lives in
//! `sassytalkie_core::floor` and is shared with the Android app. This module is
//! only the local bookkeeping: who currently owns the floor, until when, and why
//! the last local press was refused.
//!
//! ## Deadlines, not timers
//!
//! Android expresses holds as cancellable coroutine jobs (`floorHoldJob`,
//! `peerSpeakingTimeoutJob`). iOS deliberately stores **absolute expiry
//! timestamps** instead and evaluates them lazily on read. Reasons:
//!
//!   * The Swift layer already polls state at 10 Hz for the UI, so a hold that
//!     expires between polls is indistinguishable from one cancelled by a timer.
//!   * Spawning a thread per PTT event on a device that may key up hundreds of
//!     times a shift is pure overhead, and a cancelled-but-still-running thread
//!     is exactly the "ghost TX" class of bug `PttCoordinator.preAudioJob`
//!     documents on the Android side.
//!   * A deadline is idempotent: re-asserting the floor from an inbound audio
//!     frame is one atomic store, with no job to cancel and no race between a
//!     cancel and a fire.
//!
//! Every accessor therefore takes `now_ms` so the caller supplies one consistent
//! clock reading per event, and so the whole module is testable without sleeping.

use parking_lot::Mutex;
use sassytalkie_core::floor as policy;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Why a local PTT press was refused. Surfaced to Swift for a snackbar, and
/// deliberately worded to match the Android strings so support/QA see the same
/// text on both platforms.
pub const REJECT_CHANNEL_BUSY: &str = "Channel busy";
pub const REJECT_NOT_ENCRYPTED: &str = "Authenticate via QR first";
pub const REJECT_MAX_TX: &str = "Transmission stopped after safety time limit";

/// Local floor bookkeeping.
pub struct FloorState {
    /// Peer id that currently owns the floor, if any.
    owner: Mutex<Option<String>>,
    /// Absolute ms timestamp at which the current hold lapses. 0 = no hold.
    hold_until_ms: AtomicU64,
    /// Absolute ms timestamp at which the UI "peer speaking" LED should clear.
    speaking_until_ms: AtomicU64,
    /// True while THIS device is broadcasting a distress beacon.
    self_emergency: AtomicBool,
    /// Last local-press rejection reason, consumed by the UI.
    reject_reason: Mutex<Option<String>>,
}

impl FloorState {
    pub fn new() -> Self {
        Self {
            owner: Mutex::new(None),
            hold_until_ms: AtomicU64::new(0),
            speaking_until_ms: AtomicU64::new(0),
            self_emergency: AtomicBool::new(false),
            reject_reason: Mutex::new(None),
        }
    }

    /// Grant/renew the floor to `peer_id` for `hold_ms`, and light the UI LED.
    ///
    /// Called both on `OP_PTT_START_V2` and on every inbound audio frame, so a
    /// stream whose control frame was lost still occupies the floor (Android's
    /// `onPeerAudioFrame` does the same). Renewing is a plain store — an earlier
    /// hold for the same peer is simply extended.
    pub fn hold(&self, peer_id: &str, hold_ms: u64, now_ms: u64) {
        *self.owner.lock() = Some(peer_id.to_string());
        self.hold_until_ms
            .store(now_ms.saturating_add(hold_ms), Ordering::SeqCst);
        self.speaking_until_ms.store(
            now_ms.saturating_add(policy::UI_SPEAKING_MS),
            Ordering::SeqCst,
        );
    }

    /// Shorten the hold to the jitter-buffer drain window after a clean
    /// `OP_PTT_STOP_V2`, so the channel does not stay locked for the full stale
    /// hold once we know the talker finished.
    ///
    /// Only shortens: if the remaining hold is already inside the drain window we
    /// leave it, and if a *different* peer has since taken the floor we do
    /// nothing at all (mirrors Android's `if (floorPeerId == peerId)` guard).
    pub fn release_after_drain(&self, peer_id: &str, now_ms: u64) {
        let owner = self.owner.lock();
        if owner.as_deref() != Some(peer_id) {
            return;
        }
        let drain_deadline = now_ms.saturating_add(policy::DRAIN_HOLD_MS);
        // fetch_update rather than a bare store: an inbound audio frame may have
        // extended the hold between the caller's `now_ms` and here.
        let _ = self.hold_until_ms.fetch_update(
            Ordering::SeqCst,
            Ordering::SeqCst,
            |current| {
                if current > drain_deadline {
                    Some(drain_deadline)
                } else {
                    None
                }
            },
        );
    }

    /// Drop the floor immediately (local release, or session wipe).
    pub fn clear(&self) {
        *self.owner.lock() = None;
        self.hold_until_ms.store(0, Ordering::SeqCst);
        self.speaking_until_ms.store(0, Ordering::SeqCst);
    }

    /// Drop the floor only if `peer_id` still owns it.
    pub fn clear_if_owner(&self, peer_id: &str) {
        let mut owner = self.owner.lock();
        if owner.as_deref() == Some(peer_id) {
            *owner = None;
            self.hold_until_ms.store(0, Ordering::SeqCst);
        }
    }

    /// Is the floor currently occupied by anyone?
    pub fn is_held(&self, now_ms: u64) -> bool {
        self.hold_until_ms.load(Ordering::SeqCst) > now_ms
    }

    /// Current floor owner, or `None` once the hold has lapsed.
    pub fn owner(&self, now_ms: u64) -> Option<String> {
        if !self.is_held(now_ms) {
            return None;
        }
        self.owner.lock().clone()
    }

    /// Should the UI show a peer as speaking?
    pub fn peer_speaking(&self, now_ms: u64) -> bool {
        self.speaking_until_ms.load(Ordering::SeqCst) > now_ms
    }

    pub fn self_emergency(&self) -> bool {
        self.self_emergency.load(Ordering::SeqCst)
    }

    pub fn set_self_emergency(&self, active: bool) {
        self.self_emergency.store(active, Ordering::SeqCst);
    }

    /// Record why a press was refused, for the UI to pick up.
    pub fn set_reject_reason(&self, reason: &str) {
        *self.reject_reason.lock() = Some(reason.to_string());
    }

    pub fn clear_reject_reason(&self) {
        *self.reject_reason.lock() = None;
    }

    /// Read-and-clear the pending rejection reason (one-shot, like a snackbar).
    pub fn take_reject_reason(&self) -> Option<String> {
        self.reject_reason.lock().take()
    }

    /// Would a local press be refused right now? Wraps the shared policy so the
    /// emergency override lives in exactly one place.
    pub fn should_block_local(&self, now_ms: u64) -> bool {
        policy::should_block_local(self.is_held(now_ms), self.self_emergency())
    }
}

impl Default for FloorState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_700_000_000_000;

    #[test]
    fn hold_expires_on_its_own_without_a_timer() {
        let f = FloorState::new();
        f.hold("peer-a", policy::STALE_HOLD_MS, T0);
        assert!(f.is_held(T0 + 1));
        assert!(f.is_held(T0 + policy::STALE_HOLD_MS - 1));
        assert!(!f.is_held(T0 + policy::STALE_HOLD_MS));
        assert_eq!(f.owner(T0 + 1).as_deref(), Some("peer-a"));
        assert_eq!(f.owner(T0 + policy::STALE_HOLD_MS), None);
    }

    #[test]
    fn ui_led_clears_before_the_floor_does() {
        let f = FloorState::new();
        f.hold("peer-a", policy::STALE_HOLD_MS, T0);
        // The 400 ms LED lapses long before the 1500 ms floor lock — the exact
        // asymmetry that stops a cellular gap from reading as "channel free".
        assert!(!f.peer_speaking(T0 + policy::UI_SPEAKING_MS));
        assert!(f.is_held(T0 + policy::UI_SPEAKING_MS));
    }

    #[test]
    fn inbound_audio_reasserts_a_lapsing_hold() {
        let f = FloorState::new();
        f.hold("peer-a", policy::STALE_HOLD_MS, T0);
        let later = T0 + policy::STALE_HOLD_MS - 100;
        f.hold("peer-a", policy::STALE_HOLD_MS, later);
        assert!(f.is_held(T0 + policy::STALE_HOLD_MS + 100));
    }

    #[test]
    fn drain_shortens_but_never_extends_the_hold() {
        let f = FloorState::new();
        f.hold("peer-a", policy::STALE_HOLD_MS, T0);
        f.release_after_drain("peer-a", T0);
        assert!(f.is_held(T0 + policy::DRAIN_HOLD_MS - 1));
        assert!(!f.is_held(T0 + policy::DRAIN_HOLD_MS));

        // A stop arriving when less than DRAIN_HOLD_MS remains must not push the
        // deadline back out.
        let g = FloorState::new();
        g.hold("peer-a", 50, T0);
        g.release_after_drain("peer-a", T0);
        assert!(!g.is_held(T0 + 50));
    }

    #[test]
    fn drain_ignores_a_stop_from_a_peer_that_no_longer_owns_the_floor() {
        let f = FloorState::new();
        f.hold("peer-a", policy::STALE_HOLD_MS, T0);
        f.hold("peer-b", policy::STALE_HOLD_MS, T0 + 10);
        // Late STOP from A must not curtail B's floor.
        f.release_after_drain("peer-a", T0 + 20);
        assert!(f.is_held(T0 + policy::STALE_HOLD_MS));
        assert_eq!(f.owner(T0 + 100).as_deref(), Some("peer-b"));
    }

    #[test]
    fn local_emergency_beats_a_held_floor() {
        let f = FloorState::new();
        f.hold("peer-a", policy::STALE_HOLD_MS, T0);
        assert!(f.should_block_local(T0 + 1));
        f.set_self_emergency(true);
        assert!(!f.should_block_local(T0 + 1));
    }

    #[test]
    fn reject_reason_is_one_shot() {
        let f = FloorState::new();
        assert_eq!(f.take_reject_reason(), None);
        f.set_reject_reason(REJECT_CHANNEL_BUSY);
        assert_eq!(f.take_reject_reason().as_deref(), Some(REJECT_CHANNEL_BUSY));
        assert_eq!(f.take_reject_reason(), None);
    }

    #[test]
    fn clear_if_owner_only_matches_the_owner() {
        let f = FloorState::new();
        f.hold("peer-a", policy::STALE_HOLD_MS, T0);
        f.clear_if_owner("peer-b");
        assert!(f.is_held(T0 + 1));
        f.clear_if_owner("peer-a");
        assert!(!f.is_held(T0 + 1));
    }
}
