// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-IOS3PAYWALLVW8
//
//  PaywallView.swift
//  SassyTalkie — StoreKit 2 paywall (parity with Android Entitlements.GateScreen).

import SwiftUI

struct PaywallView: View {
    @ObservedObject var viewModel: SassyTalkieViewModel
    @Environment(\.presentationMode) private var presentationMode

    @State private var busy = false
    @State private var status = ""

    var body: some View {
        NavigationView {
            VStack(spacing: 20) {
                Image(systemName: "lock.fill")
                    .font(.system(size: 44))
                    .foregroundColor(.stCoral)

                Text(headline)
                    .font(.title2).bold()
                    .foregroundColor(.stTextPrimary)
                    .multilineTextAlignment(.center)

                Text(bodyCopy)
                    .font(.body)
                    .foregroundColor(.stTextSecondary)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal)

                if !status.isEmpty {
                    Text(status)
                        .font(.caption)
                        .foregroundColor(.stWarning)
                }

                Button(action: buy) {
                    HStack {
                        if busy { ProgressView().progressViewStyle(CircularProgressViewStyle(tint: .white)) }
                        Text("Unlock SassyTalkie")
                            .fontWeight(.semibold)
                    }
                    .frame(maxWidth: .infinity)
                    .padding()
                    .background(Color.stTeal)
                    .foregroundColor(.white)
                    .cornerRadius(SassyTheme.radiusMd)
                }
                .disabled(busy)

                Button("Restore Purchase") { restore() }
                    .foregroundColor(.stTeal)
                    .disabled(busy)

                Spacer()
            }
            .padding(24)
            .background(Color.stBgDark.ignoresSafeArea())
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .navigationBarLeading) {
                    Button("Back") { presentationMode.wrappedValue.dismiss() }
                        .foregroundColor(.stTeal)
                }
            }
        }
        .preferredColorScheme(.dark)
        .accentColor(.stTeal)
    }

    private var headline: String {
        if TrialStore.trialExhausted(entitled: viewModel.isEntitled) {
            return "You used your 5 free sessions"
        }
        return "This build arrived locked"
    }

    private var bodyCopy: String {
        if #available(iOS 15.0, *) {
            return "A one-time unlock keeps the radio working after the trial. Same product as Android (\(StoreKitEntitlements.productId))."
        }
        return "Purchasing needs iOS 15 or later. The 5-session trial still works on this device."
    }

    private func buy() {
        if #available(iOS 15.0, *) {
            busy = true
            Task {
                let result = await StoreKitEntitlements.purchase()
                await MainActor.run {
                    busy = false
                    switch result {
                    case .success(true):
                        viewModel.isEntitled = true
                        presentationMode.wrappedValue.dismiss()
                    case .success(false):
                        status = "Purchase cancelled or pending"
                    case .failure(let err):
                        status = err.localizedDescription
                    }
                }
            }
        } else {
            status = "Requires iOS 15+"
        }
    }

    private func restore() {
        busy = true
        StoreKitEntitlements.refresh { ok in
            busy = false
            viewModel.isEntitled = ok
            status = ok ? "Restored" : "No purchase found"
            if ok { presentationMode.wrappedValue.dismiss() }
        }
    }
}
