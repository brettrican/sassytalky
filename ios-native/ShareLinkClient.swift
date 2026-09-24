// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-IOS5SHARELNK3M
//
//  ShareLinkClient.swift
//  Mint encrypted invite links (parity with Android SessionShareLink.createShare).
//  Encrypt via shared core; POST /share with the room capability token.

import Foundation

enum ShareLinkClient {
    static let relayBase = "https://relay.sassyconsultingllc.com"
    static let appScheme = "sassy-talks"
    static let legacyScheme = "sassytalk"

    struct Minted {
        let url: String
        let httpsUrl: String
    }

    static func isAppScheme(_ scheme: String?) -> Bool {
        scheme == appScheme || scheme == legacyScheme
    }

    static func mint(sessionJSON: String, completion: @escaping (Result<Minted, Error>) -> Void) {
        guard let room = roomId(from: sessionJSON), !room.isEmpty else {
            completion(.failure(ShareError.noRoom)); return
        }
        fetchToken(room: room) { token in
            guard let token = token else {
                completion(.failure(ShareError.auth)); return
            }
            guard let sealed = encrypt(sessionJSON) else {
                completion(.failure(ShareError.encrypt)); return
            }
            postShare(room: room, token: token, blob: sealed.blob) { id in
                guard let id = id else {
                    completion(.failure(ShareError.server)); return
                }
                let https = "\(relayBase)/v/\(id)#\(sealed.key)"
                let app = "\(appScheme)://v/\(id)#\(sealed.key)"
                completion(.success(Minted(url: app, httpsUrl: https)))
            }
        }
    }

    private static func roomId(from json: String) -> String? {
        guard let data = json.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return nil
        }
        return obj["session_id"] as? String
    }

    private struct Sealed { let blob: Data; let key: String }

    private static func encrypt(_ json: String) -> Sealed? {
        let payload: String? = json.withCString { ptr in
            guard let c = sassytalkie_encrypt_share_blob(ptr) else { return nil }
            defer { sassytalkie_free_string(c) }
            return String(cString: c)
        }
        guard let payload = payload,
              let data = payload.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let b64 = obj["blob_b64"] as? String,
              let key = obj["key_b64url"] as? String,
              let blob = Data(base64Encoded: b64) else { return nil }
        return Sealed(blob: blob, key: key)
    }

    private static func fetchToken(room: String, completion: @escaping (String?) -> Void) {
        // Legacy room-only /auth: share mint may run without a peer id wired here;
        // REQUIRE_AUTH_PROOF stays off so old clients keep working.
        let enc = room.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? room
        guard let url = URL(string: "\(relayBase)/auth?room=\(enc)") else {
            completion(nil); return
        }
        PinnedURLSession.shared.dataTask(with: url) { data, _, _ in
            guard let data = data,
                  let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let token = json["token"] as? String, !token.isEmpty else {
                completion(nil); return
            }
            completion(token)
        }.resume()
    }

    private static func postShare(room: String, token: String, blob: Data, completion: @escaping (String?) -> Void) {
        let enc = room.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? room
        guard let url = URL(string: "\(relayBase)/share?room=\(enc)&burn=1") else {
            completion(nil); return
        }
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        req.setValue("application/octet-stream", forHTTPHeaderField: "Content-Type")
        req.httpBody = blob
        PinnedURLSession.shared.dataTask(with: req) { data, _, _ in
            guard let data = data,
                  let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let id = json["id"] as? String, !id.isEmpty else {
                completion(nil); return
            }
            completion(id)
        }.resume()
    }

    enum ShareError: LocalizedError {
        case noRoom, auth, encrypt, server
        var errorDescription: String? {
            switch self {
            case .noRoom: return "Session has no room id"
            case .auth: return "Could not authenticate to the relay"
            case .encrypt: return "Encrypt failed"
            case .server: return "Server returned no share id"
            }
        }
    }
}
