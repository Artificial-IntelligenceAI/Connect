import Foundation
import MessagingCore

struct ChatMessage: Identifiable {
    let id = UUID()
    let from: String
    let text: String
    let isSystem: Bool
}

enum ConnectionState: Equatable {
    case disconnected
    case connecting
    case connected
    case failed(String)
}

/// Where this client's identity and known-peer-keys get persisted. Same
/// directory works for macOS and iOS -- the app support dir is already
/// sandboxed per-app on both platforms.
private func defaultDataDirectory() -> String {
    let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
        ?? FileManager.default.temporaryDirectory
    let dir = base.appendingPathComponent("Connect", isDirectory: true)
    try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    return dir.path
}

@MainActor
final class NetworkClient: ObservableObject {
    @Published var messages: [ChatMessage] = []
    @Published var state: ConnectionState = .disconnected

    private let client = MessagingCore.ConnectClient(dataDir: defaultDataDirectory())
    private var listener: Listener?

    func connect(host: String, port: Int, displayName: String) {
        guard let portNumber = UInt16(exactly: port) else {
            state = .failed("Invalid port")
            return
        }
        let listener = Listener(owner: self)
        self.listener = listener
        client.connect(host: host, port: portNumber, displayName: displayName, listener: listener)
    }

    func send(text: String) {
        client.send(text: text)
    }

    func disconnect() {
        client.disconnect()
        listener = nil
        state = .disconnected
    }

    fileprivate func handleStateChanged(_ newState: MessagingCore.ConnectionState) {
        switch newState {
        case .disconnected:
            state = .disconnected
        case .connecting:
            state = .connecting
        case .connected:
            state = .connected
        case .failed(let reason):
            state = .failed(reason)
        }
    }

    fileprivate func handleMessage(_ message: MessagingCore.ChatMessage) {
        messages.append(ChatMessage(from: message.from, text: message.text, isSystem: message.isSystem))
    }
}

/// Bridges Rust-core callbacks (delivered on a background thread) back onto
/// the main actor, where `NetworkClient`'s published state actually lives.
private final class Listener: MessagingCore.ConnectClientListener, @unchecked Sendable {
    private weak var owner: NetworkClient?

    init(owner: NetworkClient) {
        self.owner = owner
    }

    func onStateChanged(state: MessagingCore.ConnectionState) {
        Task { @MainActor [weak owner] in
            owner?.handleStateChanged(state)
        }
    }

    func onMessage(message: MessagingCore.ChatMessage) {
        Task { @MainActor [weak owner] in
            owner?.handleMessage(message)
        }
    }
}
