// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-IOS1TRIALGATE7
//
//  TrialStore.swift
//  SassyTalkie — usage-based free trial (parity with Android TrialGate.kt).
//
//  Soft gate: UserDefaults, clearing app data resets it. Hardening would need a
//  server round-trip per join, which breaks the offline/mesh case the radio
//  exists for. Entitlements (StoreKit) remain authoritative for paying users.

import Foundation

enum TrialStore {
    static let freeSessions = 5
    private static let key = "trial_counted_sessions"
    private static let maxTracked = 32

    private static var defaults: UserDefaults { .standard }

    private static func counted() -> [String] {
        defaults.string(forKey: key)?
            .split(separator: "\n")
            .map(String.init)
            .filter { !$0.isEmpty } ?? []
    }

    static func qualifyingSessions() -> Int { counted().count }

    static func sessionsRemaining() -> Int {
        max(0, freeSessions - qualifyingSessions())
    }

    static func inTrial() -> Bool { qualifyingSessions() < freeSessions }

    /// Single predicate every gate asks: entitled OR trial remaining.
    static func mayUseRadio(entitled: Bool) -> Bool {
        entitled || inTrial()
    }

    static func noteQualifyingSession(_ sessionId: String?) {
        guard let sessionId = sessionId, !sessionId.isEmpty else { return }
        var existing = counted()
        if existing.contains(sessionId) { return }
        existing.append(sessionId)
        if existing.count > maxTracked {
            existing = Array(existing.suffix(maxTracked))
        }
        defaults.set(existing.joined(separator: "\n"), forKey: key)
    }

    static func warnThreshold(remaining: Int, entitled: Bool) -> Bool {
        !entitled && (1...2).contains(remaining)
    }

    static func shouldWarn(entitled: Bool) -> Bool {
        warnThreshold(remaining: sessionsRemaining(), entitled: entitled)
    }

    static func trialExhausted(entitled: Bool) -> Bool {
        !entitled && qualifyingSessions() >= freeSessions
    }

    static func reset() {
        defaults.removeObject(forKey: key)
    }
}
