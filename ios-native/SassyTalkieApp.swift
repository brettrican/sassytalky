// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-KYXFFMBWD247
//
//  SassyTalkieApp.swift
//  SassyTalkie
//
//  Copyright © 2025 Sassy Consulting LLC. All rights reserved.
//

import SwiftUI
import AVFoundation

@main
struct SassyTalkieApp: App {
    
    init() {
        // Request microphone permission
        requestMicrophonePermission()
    }
    
    var body: some Scene {
        WindowGroup {
            ZStack {
                Color.stBgDark.ignoresSafeArea()
                ContentView()
            }
        }
    }
    
    private func requestMicrophonePermission() {
        #if !targetEnvironment(simulator)
        AVAudioSession.sharedInstance().requestRecordPermission { granted in
            if granted {
                print("✅ Microphone permission granted")
            } else {
                print("❌ Microphone permission denied")
            }
        }
        #endif
    }
}
