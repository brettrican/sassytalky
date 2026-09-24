// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-IOS6APNSWAKE4R
//
//  PushWakePolicy.swift
//  APNs equivalent of Android FcmWakePolicy — policy only.
//
//  DEFERRED (not a silent drop):
//    Android uses FCM data messages to wake a warm WalkieService, or a
//    high-priority notification the operator taps when the process is dead.
//    iOS cannot start microphone capture from a silent push (same class of
//    restriction as Android API 34+ FGS+mic from FCM). Cold wake MUST be a
//    user-visible notification; a warm app may reconnect the relay on
//    `application(_:didReceiveRemoteNotification:)`.
//
//  Remaining owner work: Apple Developer account APNs key, relay worker
//  APNs dispatch (not FCM), and Info.plist `remote-notification` background
//  mode. Until an APNs device token exists there is nothing to POST to
//  /presence — do not invent a token. Do not add the background mode until
//  the worker actually sends wakes — unused background modes are an App
//  Store review risk.

import Foundation

enum PushWakePolicy {
    enum Path { case warmReconnect, visibleNotification }

    static func path(appIsActive: Bool) -> Path {
        appIsActive ? .warmReconnect : .visibleNotification
    }

    /// iOS never allows starting AVAudioEngine recording from a silent push.
    static func mayStartMicrophoneFromSilentPush() -> Bool { false }

    static func mayStartUIFromSilentPush() -> Bool { false }

    static let wakeCategoryId = "sassytalkie_apns_wake"
}
