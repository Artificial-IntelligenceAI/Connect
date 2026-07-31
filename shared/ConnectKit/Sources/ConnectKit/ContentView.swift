import SwiftUI
import MessagingCore
#if os(macOS)
import AppKit
#endif

/// Resigns whichever text field currently has focus. Used to let a tap
/// anywhere outside a field dismiss the keyboard, since neither platform
/// does this by default for a plain `TextField`.
private func dismissKeyboard() {
    #if os(iOS)
    UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
    #elseif os(macOS)
    NSApp.keyWindow?.makeFirstResponder(nil)
    #endif
}

enum ChatFilter: CaseIterable, Hashable {
    case all, direct, group

    var label: String {
        switch self {
        case .all: return "All"
        case .direct: return "DM"
        case .group: return "GC"
        }
    }
}

enum SelectedConversation: Equatable {
    case direct(peerIdentityKey: String, displayName: String)
    case group(groupId: String, groupName: String)

    var title: String {
        switch self {
        case .direct(_, let displayName): return displayName
        case .group(_, let groupName): return groupName
        }
    }
}

private struct ConversationEntry: Identifiable {
    let id: String
    let title: String
    let subtitle: String
    let target: SelectedConversation
}

public struct ContentView: View {
    @StateObject private var client = NetworkClient()
    @ObservedObject private var settings = AppSettings.shared

    @State private var host = "127.0.0.1"
    @State private var port = "7878"
    @State private var displayName = ""
    @State private var draft = ""
    @State private var filter: ChatFilter = .all
    @State private var searchText = ""
    @State private var selectedConversation: SelectedConversation?
    @State private var showingCreateGroup = false
    @State private var showingSettings = false
    @State private var showingInvites = false

    public init() {}

    public var body: some View {
        Group {
            switch client.state {
            case .connected, .reconnecting:
                if let selected = selectedConversation {
                    conversationView(selected)
                } else {
                    chatListView
                }
            default:
                connectView
            }
        }
        .frame(minWidth: 420, minHeight: 500)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Solarized.base3)
        .onTapGesture { dismissKeyboard() }
        .preferredColorScheme(settings.theme == .solarizedDark ? .dark : .light)
    }

    private var connectView: some View {
        VStack(spacing: 12) {
            Text("Connect to a LAN server")
                .font(.title2)
                .foregroundStyle(Solarized.base01)

            themedField(placeholder: "Server address", text: $host)
            themedField(placeholder: "Port", text: $port)
            themedField(placeholder: "Display name", text: $displayName)

            if case .failed(let reason) = client.state {
                Text(reason).foregroundStyle(Solarized.red).font(.caption)
            }

            Button("Connect") {
                guard let portNumber = Int(port), !displayName.isEmpty else { return }
                client.connect(host: host, port: portNumber, displayName: displayName)
            }
            .buttonStyle(.borderedProminent)
            .keyboardShortcut(.defaultAction)
            .disabled(client.state == .connecting)
        }
        .padding(32)
        .frame(width: 320)
    }

    // MARK: - Chat list

    private var chatListView: some View {
        HStack(spacing: 0) {
            filterSidebar
            Divider().background(Solarized.base1)
            VStack(spacing: 0) {
                if case .reconnecting(let attempt) = client.state {
                    reconnectingBanner(attempt)
                }
                listHeader
                themedField(placeholder: "Search", text: $searchText)
                    .padding(.horizontal, 12)
                    .padding(.top, 8)
                systemNoticeFeed
                conversationList
            }
        }
        .sheet(isPresented: $showingCreateGroup) {
            createGroupSheet
        }
    }

    private var filterSidebar: some View {
        VStack(spacing: 8) {
            Spacer()
            ForEach(ChatFilter.allCases, id: \.self) { option in
                Button(option.label) { filter = option }
                    .buttonStyle(.plain)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 10)
                    .background(filter == option ? Solarized.blue.opacity(0.2) : Color.clear)
                    .foregroundStyle(filter == option ? Solarized.blue : Solarized.base01)
                    .clipShape(RoundedRectangle(cornerRadius: 6))
            }
            Button {
                showingInvites = true
            } label: {
                Image(systemName: "envelope.badge")
            }
            .buttonStyle(.plain)
            .foregroundStyle(Solarized.base01)
            Button {
                showingSettings = true
            } label: {
                Image(systemName: "gearshape")
            }
            .buttonStyle(.plain)
            .foregroundStyle(Solarized.base01)
            Spacer()
        }
        .padding(8)
        .frame(width: 72)
        .background(Solarized.base2)
        .sheet(isPresented: $showingSettings) {
            SettingsView(settings: settings)
        }
        .sheet(isPresented: $showingInvites) {
            InvitesView(client: client)
        }
    }

    private var listHeader: some View {
        HStack {
            Text("Chats").font(.headline).foregroundStyle(Solarized.base01)
            Spacer()
            Button {
                showingCreateGroup = true
            } label: {
                Image(systemName: "plus.circle.fill")
            }
            .buttonStyle(.plain)
            .foregroundStyle(Solarized.blue)
        }
        .padding(12)
    }

    /// Connection-lifecycle notices (fingerprint, join/leave, TOFU
    /// warnings) are room-wide, not tied to any one conversation, so they
    /// live here on the list screen rather than inside a per-conversation
    /// view.
    private var systemNoticeFeed: some View {
        let notices = client.messages.filter { $0.conversation == .system }
        return Group {
            if !notices.isEmpty {
                ScrollView {
                    VStack(alignment: .leading, spacing: 2) {
                        ForEach(notices) { notice in
                            Text(notice.text)
                                .font(.caption2)
                                .foregroundStyle(Solarized.base1)
                        }
                    }
                    .padding(.horizontal, 12)
                    .padding(.top, 6)
                }
                .frame(maxHeight: 80)
                .scrollDismissesKeyboard(.immediately)
            }
        }
    }

    private var conversationEntries: [ConversationEntry] {
        var entries: [ConversationEntry] = []
        if filter != .group {
            for peer in client.knownPeers {
                entries.append(ConversationEntry(
                    id: "direct:\(peer.identityKey)",
                    title: peer.displayName,
                    subtitle: peer.peerId != nil ? "online" : "offline",
                    target: .direct(peerIdentityKey: peer.identityKey, displayName: peer.displayName)
                ))
            }
        }
        if filter != .direct {
            for group in client.groups {
                entries.append(ConversationEntry(
                    id: "group:\(group.groupId)",
                    title: group.name,
                    subtitle: "\(group.memberCount) member\(group.memberCount == 1 ? "" : "s")",
                    target: .group(groupId: group.groupId, groupName: group.name)
                ))
            }
        }
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        if !query.isEmpty {
            entries = entries.filter { $0.title.localizedCaseInsensitiveContains(query) }
        }
        return entries.sorted { $0.title.localizedCaseInsensitiveCompare($1.title) == .orderedAscending }
    }

    private var conversationList: some View {
        List(conversationEntries) { entry in
            Button {
                selectedConversation = entry.target
            } label: {
                VStack(alignment: .leading, spacing: 2) {
                    Text(entry.title).foregroundStyle(Solarized.base01)
                    Text(entry.subtitle).font(.caption).foregroundStyle(Solarized.base1)
                }
            }
            .buttonStyle(.plain)
        }
        .listStyle(.plain)
        .background(Solarized.base3)
        .scrollDismissesKeyboard(.immediately)
        .overlay {
            if conversationEntries.isEmpty {
                Text(searchText.isEmpty ? "No conversations yet" : "No matches")
                    .foregroundStyle(Solarized.base1)
                    .font(.caption)
            }
        }
    }

    private var createGroupSheet: some View {
        CreateGroupSheet(
            onlinePeers: client.knownPeers.filter { $0.peerId != nil },
            onCreate: { name, memberPeerIds in
                if let groupId = client.createGroup(name: name, memberPeerIds: memberPeerIds) {
                    selectedConversation = .group(groupId: groupId, groupName: name)
                }
                showingCreateGroup = false
            },
            onCancel: { showingCreateGroup = false }
        )
    }

    // MARK: - Per-conversation view

    private func conversationView(_ selected: SelectedConversation) -> some View {
        VStack(spacing: 0) {
            HStack {
                Button {
                    selectedConversation = nil
                } label: {
                    Label("Back", systemImage: "chevron.left")
                }
                .buttonStyle(.plain)
                .foregroundStyle(Solarized.blue)
                Spacer()
                Text(selected.title).font(.headline).foregroundStyle(Solarized.base01)
                Spacer()
                Color.clear.frame(width: 44) // balances the back button so the title stays centered
            }
            .padding(12)
            .background(Solarized.base2)

            if case .reconnecting(let attempt) = client.state {
                reconnectingBanner(attempt)
            }

            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 8) {
                        ForEach(messages(for: selected)) { message in
                            messageRow(message).id(message.id)
                        }
                    }
                    .padding()
                }
                .background(Solarized.base3)
                .scrollDismissesKeyboard(.immediately)
                .onChange(of: client.messages.count) { _ in
                    if let last = messages(for: selected).last {
                        proxy.scrollTo(last.id, anchor: .bottom)
                    }
                }
            }

            Divider().background(Solarized.base1)

            HStack {
                themedField(placeholder: "Message", text: $draft, onSubmit: { sendDraft(to: selected) })
                Button("Send") { sendDraft(to: selected) }
                    .disabled(draft.isEmpty)
            }
            .padding()
            .background(Solarized.base2)
        }
    }

    private func messages(for selected: SelectedConversation) -> [ChatMessage] {
        client.messages.filter { message in
            switch (message.conversation, selected) {
            case (.direct(let key), .direct(let selectedKey, _)):
                return key == selectedKey
            case (.group(let groupId, _), .group(let selectedGroupId, _)):
                return groupId == selectedGroupId
            default:
                return false
            }
        }
    }

    private func sendDraft(to selected: SelectedConversation) {
        guard !draft.isEmpty else { return }
        switch selected {
        case .direct(let peerIdentityKey, _):
            client.sendDirectMessage(peerIdentityKey: peerIdentityKey, text: draft)
        case .group(let groupId, _):
            client.sendGroupMessage(groupId: groupId, text: draft)
        }
        draft = ""
    }

    private func reconnectingBanner(_ attempt: UInt32) -> some View {
        Text("Reconnecting\u{2026} (attempt \(attempt))")
            .font(.caption)
            .foregroundStyle(Solarized.yellow)
            .frame(maxWidth: .infinity)
            .padding(6)
            .background(Solarized.base2)
    }

    private func messageRow(_ message: ChatMessage) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(message.from).font(.caption).bold().foregroundStyle(Solarized.base01)
            Text(message.text).foregroundStyle(Solarized.base00)
        }
    }

    private func themedField(
        placeholder: String,
        text: Binding<String>,
        onSubmit: (() -> Void)? = nil
    ) -> some View {
        TextField(placeholder, text: text)
            .textFieldStyle(.plain)
            .padding(8)
            .background(Solarized.base2)
            .foregroundStyle(Solarized.base00)
            .overlay(
                RoundedRectangle(cornerRadius: 6)
                    .stroke(Solarized.base1, lineWidth: 1)
            )
            .autocorrectionDisabled(!settings.autoCorrectEnabled)
            .onSubmit { onSubmit?() }
    }
}

/// Theme (dropdown) and Auto-Correct (toggle) -- the only two settings so
/// far. Reads/writes `AppSettings.shared` directly via the passed-in
/// observed instance so changes apply live and persist immediately.
private struct SettingsView: View {
    @ObservedObject var settings: AppSettings

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            Text("Settings").font(.headline).foregroundStyle(Solarized.base01)

            VStack(alignment: .leading, spacing: 6) {
                Text("Theme").foregroundStyle(Solarized.base01)
                Picker("Theme", selection: $settings.theme) {
                    ForEach(AppTheme.allCases) { theme in
                        Text(theme.rawValue).tag(theme)
                    }
                }
                .pickerStyle(.menu)
                .tint(Solarized.blue)
                .labelsHidden()
            }

            Toggle("Auto-Correct", isOn: $settings.autoCorrectEnabled)
                .tint(Solarized.blue)
                .foregroundStyle(Solarized.base01)

            Spacer()
        }
        .padding(20)
        .frame(width: 300, height: 220)
        .background(Solarized.base3)
    }
}

/// Invite an existing group's new member -- online *or* offline, unlike
/// the "+" new-group flow above, which only ever offers currently-online
/// peers at creation time. Pick a group, then pick any known peer not
/// already in it; the invite is sent via `ConnectClient.inviteToGroup`,
/// which the relay server delivers right away if they're online or holds
/// until they next join if they're not.
private struct InvitesView: View {
    @ObservedObject var client: NetworkClient
    @State private var selectedGroup: GroupSummary?

    var body: some View {
        VStack(spacing: 0) {
            header
            if let group = selectedGroup {
                invitablePeerList(for: group)
            } else {
                groupList
            }
        }
        .frame(width: 320, height: 380)
        .background(Solarized.base3)
    }

    private var header: some View {
        HStack {
            if selectedGroup != nil {
                Button {
                    selectedGroup = nil
                } label: {
                    Label("Back", systemImage: "chevron.left")
                }
                .buttonStyle(.plain)
                .foregroundStyle(Solarized.blue)
            }
            Spacer()
            Text(selectedGroup?.name ?? "Invite to a Group")
                .font(.headline)
                .foregroundStyle(Solarized.base01)
            Spacer()
            if selectedGroup != nil {
                Color.clear.frame(width: 44) // balances the back button so the title stays centered
            }
        }
        .padding(12)
        .background(Solarized.base2)
    }

    private var groupList: some View {
        Group {
            if client.groups.isEmpty {
                Spacer()
                Text("No groups yet -- create one first with the + button.")
                    .font(.caption)
                    .foregroundStyle(Solarized.base1)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 24)
                Spacer()
            } else {
                List(client.groups, id: \.groupId) { group in
                    Button {
                        selectedGroup = group
                    } label: {
                        HStack {
                            Text(group.name).foregroundStyle(Solarized.base01)
                            Spacer()
                            Text("\(group.memberCount) member\(group.memberCount == 1 ? "" : "s")")
                                .font(.caption)
                                .foregroundStyle(Solarized.base1)
                        }
                    }
                    .buttonStyle(.plain)
                }
                .listStyle(.plain)
            }
        }
    }

    private func invitablePeerList(for group: GroupSummary) -> some View {
        let currentMembers = Set(client.groupMembers(groupId: group.groupId))
        let invitablePeers = client.knownPeers.filter { !currentMembers.contains($0.identityKey) }

        return Group {
            if invitablePeers.isEmpty {
                Spacer()
                Text("Everyone you know is already in this group.")
                    .font(.caption)
                    .foregroundStyle(Solarized.base1)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 24)
                Spacer()
            } else {
                List(invitablePeers, id: \.identityKey) { peer in
                    Button {
                        client.inviteToGroup(groupId: group.groupId, peerIdentityKey: peer.identityKey)
                    } label: {
                        HStack {
                            Text(peer.displayName).foregroundStyle(Solarized.base01)
                            Spacer()
                            Text(peer.peerId != nil ? "online" : "offline")
                                .font(.caption)
                                .foregroundStyle(Solarized.base1)
                        }
                    }
                    .buttonStyle(.plain)
                }
                .listStyle(.plain)
            }
        }
    }
}

/// A minimal group-creation flow: name + pick from currently-online known
/// peers. Membership is fixed at creation (see `ConnectClient.createGroup`'s
/// v1 limitations) so there's deliberately no "invite offline" affordance.
private struct CreateGroupSheet: View {
    let onlinePeers: [KnownPeer]
    let onCreate: (String, [String]) -> Void
    let onCancel: () -> Void

    @State private var name = ""
    @State private var selectedPeerIds: Set<String> = []

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("New group chat").font(.headline).foregroundStyle(Solarized.base01)

            TextField("Group name", text: $name)
                .textFieldStyle(.roundedBorder)
                .autocorrectionDisabled(!AppSettings.shared.autoCorrectEnabled)

            Text("Members (online now)").font(.caption).foregroundStyle(Solarized.base1)
            if onlinePeers.isEmpty {
                Text("No one else is online right now.")
                    .font(.caption)
                    .foregroundStyle(Solarized.base1)
            }
            List(onlinePeers, id: \.identityKey) { peer in
                if let peerId = peer.peerId {
                    Button {
                        if selectedPeerIds.contains(peerId) {
                            selectedPeerIds.remove(peerId)
                        } else {
                            selectedPeerIds.insert(peerId)
                        }
                    } label: {
                        HStack {
                            Text(peer.displayName).foregroundStyle(Solarized.base01)
                            Spacer()
                            if selectedPeerIds.contains(peerId) {
                                Image(systemName: "checkmark").foregroundStyle(Solarized.blue)
                            }
                        }
                    }
                    .buttonStyle(.plain)
                }
            }
            .listStyle(.plain)
            .frame(minHeight: 120)
            .scrollDismissesKeyboard(.immediately)

            HStack {
                Button("Cancel", action: onCancel)
                Spacer()
                Button("Create") {
                    onCreate(name, Array(selectedPeerIds))
                }
                .buttonStyle(.borderedProminent)
                .disabled(name.isEmpty || selectedPeerIds.isEmpty)
            }
        }
        .padding(20)
        .frame(width: 340, height: 380)
        .background(Solarized.base3)
        .onTapGesture { dismissKeyboard() }
    }
}
