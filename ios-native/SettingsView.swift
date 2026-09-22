// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-FVBMZ4QFMVSN
//
//  SettingsView.swift
//  SassyTalkie
//
//  Copyright © 2025 Sassy Consulting LLC. All rights reserved.
//

import SwiftUI
import UIKit

struct SettingsView: View {
    @ObservedObject var viewModel: SassyTalkieViewModel
    @Environment(\.presentationMode) var presentationMode

    var body: some View {
        NavigationView {
            Form {
                Section(header: Text("ABOUT")) {
                    diagRow("Version", viewModel.version)

                    HStack {
                        Text("Status")
                        Spacer()
                        Text(viewModel.statusText)
                            .foregroundColor(viewModel.isConnected ? .stOnline : .stTextMuted)
                    }
                }

                Section(header: Text("CHANNEL")) {
                    HStack {
                        Text("Current Channel")
                        Spacer()
                        Text(String(format: "%02d", viewModel.channel))
                            .font(SassyTheme.mono(17, .semibold))
                            .foregroundColor(.stTeal)
                    }
                }

                // Cross-platform pairing. Encryption is mandatory, so a channel is
                // silent until host + joiner share a key: one device hosts (shows a
                // QR) and the other scans it. The QR works across iOS, Android, and
                // desktop. Dismiss settings first, then drive the sheet from the
                // root view (avoids presenting a sheet over this sheet).
                Section(header: Text("PAIRING"),
                        footer: Text(viewModel.isPaired
                                     ? "Encrypted on channel \(String(format: "%02d", viewModel.channel))."
                                     : "Not paired — host or join a channel to enable audio.")) {
                    Button(action: {
                        presentationMode.wrappedValue.dismiss()
                        DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) {
                            viewModel.hostChannel()
                        }
                    }) {
                        Label("Host This Channel (show QR)", systemImage: "qrcode")
                            .foregroundColor(.stTeal)
                    }

                    Button(action: {
                        presentationMode.wrappedValue.dismiss()
                        DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) {
                            viewModel.showingScanner = true
                        }
                    }) {
                        Label("Scan QR to Join", systemImage: "qrcode.viewfinder")
                            .foregroundColor(.stTeal)
                    }
                }

                Section(header: Text("AUDIO")) {
                    Text("Audio configuration is automatic")
                        .font(.caption)
                        .foregroundColor(.stTextMuted)
                }

                // On-the-go diagnostics — a read-only field dump for field
                // testing a shipped (release) build. No keys or peer identifiers,
                // safe to read aloud / screenshot in a support thread.
                Section(header: Text("DIAGNOSTICS")) {
                    diagRow("Device", UIDevice.current.model)
                    diagRow("iOS", UIDevice.current.systemVersion)
                    diagRow("Channel", String(format: "%02d", viewModel.channel))
                    diagRow("Connection", viewModel.statusText)
                    diagRow("Transmitting", viewModel.isTransmitting ? "yes" : "no")
                    diagRow("Receiving", viewModel.isReceiving ? "yes" : "no")
                    diagRow("Paired", viewModel.isPaired ? "yes" : "no")
                    diagRow("Trial left", "\(TrialStore.sessionsRemaining())")
                    diagRow("Entitled", viewModel.isEntitled ? "yes" : "no")
                }

                Section(header: Text("LICENSE")) {
                    Button("Upgrade / Restore") {
                        presentationMode.wrappedValue.dismiss()
                        DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) {
                            viewModel.showingPaywall = true
                        }
                    }
                    .foregroundColor(.stTeal)
                }

                Section(header: Text("TECHNICAL AUDIT"),
                        footer: Text("technical audit export — not a legal chain of custody / not court-certified evidence")) {
                    Button("Export technical audit") {
                        guard let json = viewModel.exportAuditJson() else { return }
                        let sheet = UIActivityViewController(activityItems: [json], applicationActivities: nil)
                        UIApplication.shared.windows.first?.rootViewController?.present(sheet, animated: true)
                    }
                    Button("Clear session keys") {
                        viewModel.wipeSession()
                    }
                    .foregroundColor(.red)
                }

                Section(header: Text("INFO")) {
                    Link("Privacy Policy", destination: URL(string: "https://sassyconsultingllc.github.io/sassy-talks/privacy-policy.html")!)
                    Link("Support", destination: URL(string: "https://sassyconsultingllc.github.io/sassy-talks/support.html")!)
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .navigationBarTrailing) {
                    Button("Done") {
                        presentationMode.wrappedValue.dismiss()
                    }
                    .foregroundColor(.stTeal)
                }
            }
        }
        // Force dark so the grouped Form renders on slate-dark surfaces
        // (iOS-14-safe — .scrollContentBackground is iOS 16+). Teal accent
        // tints links + controls to match the Tauri reference.
        .preferredColorScheme(.dark)
        .accentColor(.stTeal)
    }

    /// Label / mono-value row used across the settings + diagnostics sections.
    private func diagRow(_ label: String, _ value: String) -> some View {
        HStack {
            Text(label)
            Spacer()
            Text(value)
                .font(SassyTheme.mono(14))
                .foregroundColor(.stTextSecondary)
        }
    }
}

struct SettingsView_Previews: PreviewProvider {
    static var previews: some View {
        SettingsView(viewModel: SassyTalkieViewModel())
    }
}
