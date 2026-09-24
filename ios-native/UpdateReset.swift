// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-IOS8UPDRESET5N
//
//  UpdateReset.swift
//  Parity with Android UpdateReset.kt: an in-place update keeps prior state
//  while the binary changes. Session keys / profile / entitlement stay.
//  Derived caches would be wiped here; iOS currently has none on disk.

import Foundation

enum UpdateReset {
    private static let key = "last_marketing_version"

    static func runIfNeeded() {
        let current = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? ""
        let last = UserDefaults.standard.string(forKey: key)
        if last == nil {
            UserDefaults.standard.set(current, forKey: key)
            return
        }
        if last != current {
            UserDefaults.standard.set(current, forKey: key)
        }
    }
}
