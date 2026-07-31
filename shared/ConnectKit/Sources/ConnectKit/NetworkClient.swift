import Foundation
import MessagingCore
import UserNotifications
#if os(macOS)
import AppKit
#elseif os(iOS)
import UIKit
#endif

/// True while this app is the active, frontmost app -- used to suppress
/// notifications for messages the user is already looking at.
private func isAppActive() -> Bool {
    #if os(macOS)
    return NSApplication.shared.isActive
    #elseif os(iOS)
    return UIApplication.shared.applicationState == .active
    #else
    return true
    #endif
}

/// Local notifications only -- there's no push infrastructure (no APNs
/// registration, no server-side offline delivery), so these only fire
/// while this process is still alive in the background. If the OS has
/// fully terminated the app, nothing will notify you until you reopen it.
private func notifyNewMessage(conversationKey: String, title: String, body: String) {
    let content = UNMutableNotificationContent()
    content.title = title
    content.body = body
    content.sound = .default
    let request = UNNotificationRequest(identifier: conversationKey, content: content, trigger: nil)
    UNUserNotificationCenter.current().add(request)
}

enum Conversation: Equatable {
    case system
    case direct(peerIdentityKey: String)
    case group(groupId: String, groupName: String)
}

struct ChatMessage: Identifiable {
    let id = UUID()
    let from: String
    let text: String
    let conversation: Conversation
}

enum ConnectionState: Equatable {
    case disconnected
    case connecting
    case connected
    case reconnecting(attempt: UInt32)
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
    @Published var knownPeers: [KnownPeer] = []
    @Published var groups: [GroupSummary] = []

    private let client = MessagingCore.ConnectClient(dataDir: defaultDataDirectory())
    private var listener: Listener?

    init() {
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge]) { _, _ in }
    }

    func connect(host: String, port: Int, displayName: String) {
        guard let portNumber = UInt16(exactly: port) else {
            state = .failed("Invalid port")
            return
        }
        let listener = Listener(owner: self)
        self.listener = listener
        client.connect(host: host, port: portNumber, displayName: displayName, listener: listener)
    }

    func sendDirectMessage(peerIdentityKey: String, text: String) {
        client.sendDirectMessage(peerIdentityKey: peerIdentityKey, text: text)
    }

    /// Creates a group with `memberPeerIds` (currently-online peers only)
    /// and invites each of them. Returns the new group's id, or `nil` if
    /// not connected or none of the member ids resolved to a known peer.
    func createGroup(name: String, memberPeerIds: [String]) -> String? {
        client.createGroup(name: name, memberPeerIds: memberPeerIds)
    }

    func sendGroupMessage(groupId: String, text: String) {
        client.sendGroupMessage(groupId: groupId, text: text)
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
        case .reconnecting(let attempt):
            state = .reconnecting(attempt: attempt)
        case .failed(let reason):
            state = .failed(reason)
        }
    }

    fileprivate func handleMessage(_ message: MessagingCore.ChatMessage) {
        let conversation: Conversation
        switch message.conversation {
        case .system:
            conversation = .system
        case .direct(let peerIdentityKey):
            conversation = .direct(peerIdentityKey: peerIdentityKey)
        case .group(let groupId, let groupName):
            conversation = .group(groupId: groupId, groupName: groupName)
        }
        messages.append(ChatMessage(from: message.from, text: message.text, conversation: conversation))

        // Any message (a chat message or a system notice like "X joined"
        // or "Added to group") is a reasonable cue that the contacts/group
        // lists might have changed -- there's no finer-grained push signal
        // for this yet, and list sizes are small enough that refreshing
        // every time is cheap.
        knownPeers = client.listKnownPeers()
        groups = client.listGroups()

        // Only notify for actual chat content, and only while the app
        // isn't already frontmost -- no point interrupting someone already
        // looking at the conversation.
        if !isAppActive() {
            switch conversation {
            case .system:
                break
            case .direct(let peerIdentityKey):
                notifyNewMessage(conversationKey: "direct:\(peerIdentityKey)", title: message.from, body: message.text)
            case .group(let groupId, let groupName):
                notifyNewMessage(conversationKey: "group:\(groupId)", title: groupName, body: "\(message.from): \(message.text)")
            }
        }
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
