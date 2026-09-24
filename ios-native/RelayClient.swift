// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-GDROUYFZ4F2D
//
//  RelayClient.swift
//  SassyTalkie — Cloudflare relay WebSocket client (remote peers).
//  Copyright © 2025 Sassy Consulting LLC. All rights reserved.
//
//  The socket lives here (URLSessionWebSocketTask); the Rust core supplies the
//  room id, seals/opens audio frames, and builds heartbeats over the C FFI —
//  mirroring the android-native queue-bridge model so the relay wire bytes are
//  byte-identical to the phone (and the desktop, verified in tauri-desktop).
//
//  Wire protocol (must match android-native / tauri-desktop):
//    1. GET  https://relay.sassyconsultingllc.com/auth?room=<room>&peer=<peer>
//         → {"token": "..."}  (peer= when known; legacy room-only still accepted)
//    2. wss://relay.sassyconsultingllc.com/ws?room=&token=&device=&peer=&client_id=
//         (+ catchup=1 on reconnect)
//    3. binary WS messages = sealed core::wire frames (+ OP_HEARTBEAT TLV ~every 2s)
//
//  Audio TX already tees sealed frames into the Rust relay outbound queue while
//  `sassytalkie_relay_set_active(true)`; this client drains that queue and feeds
//  inbound binary frames back through `sassytalkie_relay_on_message`.

import Foundation
import UIKit

final class RelayClient: NSObject {

    private static let httpsBase = "https://relay.sassyconsultingllc.com"
    private static let wssBase = "wss://relay.sassyconsultingllc.com"
    private static let heartbeatInterval: TimeInterval = 2.0
    private static let drainInterval: TimeInterval = 0.01   // 10 ms outbound poll
    private static let reconnectDelay: TimeInterval = 3.0

    private let session = PinnedURLSession.shared
    private var task: URLSessionWebSocketTask?
    private var heartbeatTimer: Timer?
    private var drainTimer: Timer?
    private var running = false
    /// After the first successful dial, reconnects request ?catchup=1.
    private var hasCompletedHandshake = false

    private let peerId = UIDevice.current.identifierForVendor?.uuidString ?? UUID().uuidString
    private let deviceName = UIDevice.current.name

    /// Connect to the relay for the current paired session. No-op if unpaired.
    func connect() {
        guard !running else { return }
        guard let room = Self.relayRoomId() else {
            print("Relay: not paired (no room id) — skipping connect")
            return
        }
        running = true
        fetchToken(room: room) { [weak self] token in
            guard let self = self, self.running, let token = token else {
                self?.running = false
                return
            }
            DispatchQueue.main.async { self.openSocket(room: room, token: token) }
        }
    }

    func disconnect() {
        running = false
        sassytalkie_relay_set_active(false)
        teardownTimers()
        task?.cancel(with: .goingAway, reason: nil)
        task = nil
    }

    // MARK: - Private

    private static func relayRoomId() -> String? {
        guard let c = sassytalkie_relay_room_id() else { return nil }
        defer { sassytalkie_free_string(c) }
        return String(cString: c)
    }

    private func fetchToken(room: String, completion: @escaping (String?) -> Void) {
        var comps = URLComponents(string: "\(Self.httpsBase)/auth")
        comps?.queryItems = [
            URLQueryItem(name: "room", value: room),
            URLQueryItem(name: "peer", value: peerId),
        ]
        guard let url = comps?.url else {
            completion(nil); return
        }
        session.dataTask(with: url) { data, _, _ in
            guard let data = data,
                  let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let token = json["token"] as? String, !token.isEmpty else {
                completion(nil); return
            }
            completion(token)
        }.resume()
    }

    private func openSocket(room: String, token: String) {
        func enc(_ s: String) -> String {
            s.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? s
        }
        let clientId = UUID().uuidString
        var urlStr = "\(Self.wssBase)/ws?room=\(enc(room))&token=\(enc(token))"
            + "&device=\(enc(deviceName))&peer=\(enc(peerId))&client_id=\(enc(clientId))"
        if hasCompletedHandshake {
            urlStr += "&catchup=1"
        }
        guard let url = URL(string: urlStr) else { running = false; return }

        let task = session.webSocketTask(with: url)
        self.task = task
        task.resume()
        hasCompletedHandshake = true
        sassytalkie_relay_set_active(true)
        receiveLoop()
        startTimers()
        print("Relay: connected to room \(room)")
    }

    private func receiveLoop() {
        task?.receive { [weak self] result in
            guard let self = self, self.running else { return }
            switch result {
            case .failure(let err):
                print("Relay: receive error \(err) — reconnecting")
                self.reconnect()
            case .success(let message):
                if case .data(let data) = message, !data.isEmpty {
                    data.withUnsafeBytes { (raw: UnsafeRawBufferPointer) in
                        if let base = raw.baseAddress {
                            _ = sassytalkie_relay_on_message(
                                base.assumingMemoryBound(to: UInt8.self), data.count)
                        }
                    }
                }
                self.receiveLoop()
            }
        }
    }

    private func startTimers() {
        heartbeatTimer = Timer.scheduledTimer(withTimeInterval: Self.heartbeatInterval, repeats: true) { [weak self] _ in
            self?.sendHeartbeat()
        }
        drainTimer = Timer.scheduledTimer(withTimeInterval: Self.drainInterval, repeats: true) { [weak self] _ in
            self?.drainOutbound()
        }
    }

    private func teardownTimers() {
        heartbeatTimer?.invalidate(); heartbeatTimer = nil
        drainTimer?.invalidate(); drainTimer = nil
    }

    /// Drain every sealed frame queued by the Rust TX path this tick and send it.
    private func drainOutbound() {
        var len = 0
        while let ptr = sassytalkie_relay_poll_outbound(&len), len > 0 {
            let data = Data(bytes: ptr, count: len)
            sassytalkie_free_bytes(ptr, len)
            task?.send(.data(data)) { err in
                if let err = err { print("Relay: send error \(err)") }
            }
        }
    }

    private func sendHeartbeat() {
        var len = 0
        guard let ptr = sassytalkie_relay_heartbeat_frame(&len), len > 0 else { return }
        let data = Data(bytes: ptr, count: len)
        sassytalkie_free_bytes(ptr, len)
        task?.send(.data(data)) { _ in }
    }

    private func reconnect() {
        guard running else { return }
        sassytalkie_relay_set_active(false)
        teardownTimers()
        task = nil
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.reconnectDelay) { [weak self] in
            guard let self = self, self.running else { return }
            self.running = false   // let connect() run the full dial again
            self.connect()
        }
    }
}
