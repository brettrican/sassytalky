// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-MFQGR2L4IFMG
//
//  ContentView.swift
//  SassyTalkie
//
//  Copyright © 2025 Sassy Consulting LLC. All rights reserved.
//

import SwiftUI

struct ContentView: View {
    @StateObject private var viewModel = SassyTalkieViewModel()
    
    var body: some View {
        NavigationView {
            VStack(spacing: 0) {
                // Header
                headerView

                Spacer()

                // Status
                statusView

                Spacer()

                // Channel selector
                channelSelector

                Spacer()

                // PTT Button
                pttButton

                Spacer()
            }
            .padding()
            .background(Color.stBgDark.ignoresSafeArea())
            .navigationTitle("SassyTalkie")
            .navigationBarTitleDisplayMode(.inline)
            .navigationBarItems(leading:
                Button(action: { viewModel.showingScanner = true }) {
                    Image(systemName: viewModel.isPaired ? "lock.fill" : "lock.open")
                        .foregroundColor(viewModel.isPaired ? .stOnline : .stCoral)
                }
            )
            .navigationBarItems(trailing:
                Button(action: { viewModel.showingSettings.toggle() }) {
                    Image(systemName: "gear")
                        .foregroundColor(.stTeal)
                }
            )
            .sheet(isPresented: $viewModel.showingSettings) {
                SettingsView(viewModel: viewModel)
            }
            .sheet(isPresented: $viewModel.showingScanner) {
                QRScannerView { code in
                    // Returns 0 on a malformed/expired QR; the scanner dismisses
                    // either way and the user can retry from the lock button.
                    _ = viewModel.importSessionQR(code)
                }
            }
            // Universal Link: tapping an invite https://relay…/v/<id>#<key> opens
            // the app here. Fetch + decrypt + pair via the shared core, no scan.
            .onOpenURL { url in
                viewModel.importFromShareURL(url)
            }
            .sheet(isPresented: $viewModel.showingHostQR) {
                if let json = viewModel.hostQRJSON {
                    HostQRView(json: json, channel: viewModel.channel)
                }
            }
            .sheet(isPresented: $viewModel.showingPaywall) {
                PaywallView(viewModel: viewModel)
            }
        }
    }
    
    // MARK: - Header
    
    private var headerView: some View {
        VStack(spacing: 8) {
            // Brand title — blue→purple gradient, matching the Tauri reference
            // (app.css --gradient-primary). Overlay+mask keeps this iOS 14-safe
            // (foregroundStyle gradients require iOS 15).
            Text("Sassy-Talk")
                .font(SassyTheme.ui(34, .bold))
                .opacity(0)
                .overlay(
                    SassyTheme.gradientPrimary
                        .mask(Text("Sassy-Talk").font(SassyTheme.ui(34, .bold)))
                )

            Text("Walkie-Talkie  ·  v\(viewModel.version)")
                .font(.caption)
                .foregroundColor(.stTextMuted)
        }
        .padding(.top, 20)
    }
    
    // MARK: - Status
    
    private var statusView: some View {
        VStack(spacing: 12) {
            // Connection status
            HStack {
                Circle()
                    .fill(viewModel.statusColor)
                    .frame(width: 12, height: 12)

                Text(viewModel.statusText)
                    .font(.headline)
                    .foregroundColor(.stTextPrimary)
            }

            // Encryption / pairing status. Encryption is mandatory: until a QR is
            // scanned there is no session key, so the device can neither transmit
            // nor decode. Tap to open the scanner.
            Button(action: { viewModel.showingScanner = true }) {
                HStack(spacing: 6) {
                    Image(systemName: viewModel.isPaired ? "lock.fill" : "lock.open")
                    Text(viewModel.isPaired
                         ? "Encrypted · ch \(String(format: "%02d", viewModel.channel))"
                         : "Tap to scan pairing QR")
                        .font(.caption)
                }
                .foregroundColor(viewModel.isPaired ? .stOnline : .stCoral)
            }

            if let warning = viewModel.trialWarning {
                Button(action: { viewModel.showingPaywall = true }) {
                    HStack {
                        Text(warning)
                            .font(.caption)
                            .foregroundColor(.stTextMuted)
                        Spacer()
                        Text("Upgrade")
                            .font(.caption.weight(.semibold))
                            .foregroundColor(.stTeal)
                    }
                    .padding(10)
                    .background(RoundedRectangle(cornerRadius: SassyTheme.radiusSm).fill(Color.stBgMedium))
                }
            }

            if let reject = viewModel.pttRejectText {
                Text(reject)
                    .font(.caption)
                    .foregroundColor(.stCoral)
                    .onAppear {
                        DispatchQueue.main.asyncAfter(deadline: .now() + 2.5) {
                            viewModel.pttRejectText = nil
                        }
                    }
            }

            // State indicator
            if viewModel.isTransmitting {
                Text("TRANSMITTING")
                    .font(.system(size: 24, weight: .bold))
                    .foregroundColor(.stCoral)
                    .padding(.horizontal, 20)
                    .padding(.vertical, 10)
                    .background(
                        RoundedRectangle(cornerRadius: SassyTheme.radiusSm)
                            .fill(Color.stCoral.opacity(0.2))
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: SassyTheme.radiusSm)
                            .stroke(Color.stCoral, lineWidth: 2)
                    )
            } else if viewModel.isReceiving {
                Text("RECEIVING")
                    .font(.system(size: 24, weight: .bold))
                    .foregroundColor(.stTeal)
                    .padding(.horizontal, 20)
                    .padding(.vertical, 10)
                    .background(
                        RoundedRectangle(cornerRadius: SassyTheme.radiusSm)
                            .fill(Color.stTeal.opacity(0.2))
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: SassyTheme.radiusSm)
                            .stroke(Color.stTeal, lineWidth: 2)
                    )
            }
        }
    }
    
    // MARK: - Channel Selector
    
    private var channelSelector: some View {
        VStack(spacing: 12) {
            Text("CHANNEL")
                .font(.caption)
                .foregroundColor(.stTextSecondary)

            HStack(spacing: 20) {
                Button(action: { viewModel.decrementChannel() }) {
                    Image(systemName: "minus.circle.fill")
                        .font(.system(size: 32))
                        .foregroundColor(.stTeal)
                }

                Text(String(format: "%02d", viewModel.channel))
                    .font(SassyTheme.mono(48, .bold))
                    .foregroundColor(.stTeal)
                    .frame(width: 100)

                Button(action: { viewModel.incrementChannel() }) {
                    Image(systemName: "plus.circle.fill")
                        .font(.system(size: 32))
                        .foregroundColor(.stTeal)
                }
            }
        }
    }
    
    // MARK: - PTT Button
    
    private var pttButton: some View {
        Button(action: {}) {
            ZStack {
                Circle()
                    .fill(viewModel.isPTTPressed ? Color.stCoral : Color.stBgLight)
                    .frame(width: 200, height: 200)
                    .overlay(
                        Circle()
                            .stroke(viewModel.isPTTPressed ? Color.stCoral : Color.stTeal, lineWidth: 4)
                    )
                    // Teal glow idle, coral glow while transmitting — mirrors the
                    // Tauri CTA's --shadow-glow-* effect.
                    .shadow(color: viewModel.isPTTPressed ? Color.stCoral.opacity(0.6) : Color.stTeal.opacity(0.35), radius: 20)

                VStack(spacing: 8) {
                    Image(systemName: "mic.fill")
                        .font(.system(size: 48))
                        .foregroundColor(viewModel.isPTTPressed ? .white : .stTeal)

                    Text("PUSH TO TALK")
                        .font(.system(size: 14, weight: .bold))
                        .foregroundColor(viewModel.isPTTPressed ? .white : .stTeal)
                }
            }
        }
        .buttonStyle(PTTButtonStyle(viewModel: viewModel))
        .padding(.bottom, 40)
    }
}

// MARK: - PTT Button Style

struct PTTButtonStyle: ButtonStyle {
    @ObservedObject var viewModel: SassyTalkieViewModel
    
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? 0.95 : 1.0)
            .animation(.spring(response: 0.2), value: configuration.isPressed)
            .onChange(of: configuration.isPressed) { pressed in
                if pressed {
                    viewModel.pttPress()
                } else {
                    viewModel.pttRelease()
                }
            }
    }
}

// MARK: - Color Extension

extension Color {
    init(hex: String) {
        let scanner = Scanner(string: hex)
        var rgb: UInt64 = 0
        scanner.scanHexInt64(&rgb)
        
        let r = Double((rgb >> 16) & 0xFF) / 255.0
        let g = Double((rgb >> 8) & 0xFF) / 255.0
        let b = Double(rgb & 0xFF) / 255.0
        
        self.init(red: r, green: g, blue: b)
    }
}

// MARK: - Preview

struct ContentView_Previews: PreviewProvider {
    static var previews: some View {
        ContentView()
    }
}
