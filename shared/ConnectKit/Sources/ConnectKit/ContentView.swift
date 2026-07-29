import SwiftUI

public struct ContentView: View {
    @StateObject private var client = NetworkClient()

    @State private var host = "127.0.0.1"
    @State private var port = "7878"
    @State private var displayName = ""
    @State private var draft = ""

    public init() {}

    public var body: some View {
        Group {
            switch client.state {
            case .connected, .reconnecting:
                chatView
            default:
                connectView
            }
        }
        .frame(minWidth: 420, minHeight: 500)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Solarized.base3)
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

    private var chatView: some View {
        VStack(spacing: 0) {
            if case .reconnecting(let attempt) = client.state {
                Text("Reconnecting\u{2026} (attempt \(attempt))")
                    .font(.caption)
                    .foregroundStyle(Solarized.yellow)
                    .frame(maxWidth: .infinity)
                    .padding(6)
                    .background(Solarized.base2)
            }
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 8) {
                        ForEach(client.messages) { message in
                            messageRow(message).id(message.id)
                        }
                    }
                    .padding()
                }
                .background(Solarized.base3)
                .onChange(of: client.messages.count) { _ in
                    if let last = client.messages.last {
                        proxy.scrollTo(last.id, anchor: .bottom)
                    }
                }
            }

            Divider().background(Solarized.base1)

            // Disabled until the chat-list GUI exists: sending now needs a
            // peer or group to target, which this single free-text
            // composer has no way to express.
            HStack {
                themedField(placeholder: "Message", text: $draft, onSubmit: sendDraft)
                Button("Send", action: sendDraft)
                    .disabled(true)
            }
            .padding()
            .background(Solarized.base2)
        }
    }

    private func messageRow(_ message: ChatMessage) -> some View {
        Group {
            if message.conversation == .system {
                Text(message.text)
                    .font(.caption)
                    .foregroundStyle(Solarized.base1)
            } else {
                VStack(alignment: .leading, spacing: 2) {
                    Text(message.from).font(.caption).bold().foregroundStyle(Solarized.base01)
                    Text(message.text).foregroundStyle(Solarized.base00)
                }
            }
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
            .onSubmit { onSubmit?() }
    }

    private func sendDraft() {
        // No-op for now -- see the composer's disabled state above.
    }
}
