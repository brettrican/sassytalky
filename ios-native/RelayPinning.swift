// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-IOS4TLSPINN9KQ
//
//  RelayPinning.swift
//  SPKI pins for relay.sassyconsultingllc.com — values come from the shared
//  core (`sassytalkie_tls_pins_json`), same set Android RelayTlsPins.kt uses.
//  Mismatch is fail-closed when pinning is enabled.

import Foundation
import Security
import CryptoKit

final class RelayPinningDelegate: NSObject, URLSessionDelegate {
    static let host = "relay.sassyconsultingllc.com"

    private let pins: Set<String>
    private let enabled: Bool

    override init() {
        self.enabled = sassytalkie_tls_pinning_enabled()
        var loaded = Set<String>()
        if let c = sassytalkie_tls_pins_json() {
            defer { sassytalkie_free_string(c) }
            let json = String(cString: c)
            if let data = json.data(using: .utf8),
               let arr = try? JSONSerialization.jsonObject(with: data) as? [String] {
                loaded = Set(arr)
            }
        }
        self.pins = loaded
        super.init()
    }

    func urlSession(_ session: URLSession,
                    didReceive challenge: URLAuthenticationChallenge,
                    completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
        guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
              let trust = challenge.protectionSpace.serverTrust else {
            completionHandler(.performDefaultHandling, nil)
            return
        }
        let host = challenge.protectionSpace.host
        if host != Self.host {
            completionHandler(.performDefaultHandling, nil)
            return
        }
        if !enabled || pins.count < 2 {
            completionHandler(.performDefaultHandling, nil)
            return
        }

        var secResult = SecTrustResultType.invalid
        let evaluated = SecTrustEvaluate(trust, &secResult) == errSecSuccess
            && (secResult == .unspecified || secResult == .proceed)
        guard evaluated else {
            completionHandler(.cancelAuthenticationChallenge, nil)
            return
        }

        let presented = chainSPKIPins(trust)
        let match = !presented.isEmpty && presented.contains(where: { pins.contains($0) })
        if match {
            completionHandler(.useCredential, URLCredential(trust: trust))
        } else {
            NSLog("SassyTalkie TLS: pin mismatch for %@ (fail-closed)", host)
            completionHandler(.cancelAuthenticationChallenge, nil)
        }
    }

    private func chainSPKIPins(_ trust: SecTrust) -> [String] {
        var out: [String] = []
        let count = SecTrustGetCertificateCount(trust)
        for i in 0..<count {
            guard let cert = SecTrustGetCertificateAtIndex(trust, i),
                  let pin = spkiSHA256Base64(cert) else { continue }
            out.append(pin)
        }
        return out
    }

    /// SHA-256 of SubjectPublicKeyInfo, standard base64 (OkHttp `sha256/` form).
    private func spkiSHA256Base64(_ cert: SecCertificate) -> String? {
        guard let key = SecCertificateCopyKey(cert) else { return nil }
        var error: Unmanaged<CFError>?
        guard let raw = SecKeyCopyExternalRepresentation(key, &error) as Data? else { return nil }
        let attrs = SecKeyCopyAttributes(key) as? [String: Any]
        let type = attrs?[kSecAttrKeyType as String] as? String
        let bits = (attrs?[kSecAttrKeySizeInBits as String] as? Int) ?? 0
        guard let spki = wrapSPKI(rawKey: raw, type: type, bits: bits) else { return nil }
        let hash = SHA256.hash(data: spki)
        return Data(hash).base64EncodedString()
    }

    /// Reconstruct SPKI DER from SecKey's raw export. GTS WE* are P-256 ECDSA;
    /// WR* are RSA-2048. Anything else is skipped (fail-closed if no pin hits).
    private func wrapSPKI(rawKey: Data, type: String?, bits: Int) -> Data? {
        if type == (kSecAttrKeyTypeECSECPrimeRandom as String) {
            // P-256 SubjectPublicKeyInfo header + uncompressed point (0x04 ‖ X ‖ Y).
            let header = Data([
                0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01,
                0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
            ])
            return header + rawKey
        }
        if type == (kSecAttrKeyTypeRSA as String) && bits == 2048 {
            let header = Data([
                0x30, 0x82, 0x01, 0x22, 0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86,
                0xf7, 0x0d, 0x01, 0x01, 0x01, 0x05, 0x00, 0x03, 0x82, 0x01, 0x0f, 0x00,
            ])
            return header + rawKey
        }
        return nil
    }
}

enum PinnedURLSession {
    static let shared: URLSession = {
        let config = URLSessionConfiguration.default
        config.timeoutIntervalForRequest = 15
        let delegate = RelayPinningDelegate()
        return URLSession(configuration: config, delegate: delegate, delegateQueue: nil)
    }()
}
