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
            case .connected:
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
            .keyboardShortcut(.defaultAction)
            .disabled(client.state == .connecting)
        }
        .padding(32)
        .frame(width: 320)
    }

    private var chatView: some View {
        VStack(spacing: 0) {
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

            HStack {
                themedField(placeholder: "Message", text: $draft, onSubmit: sendDraft)
                Button("Send", action: sendDraft)
                    .disabled(draft.isEmpty)
            }
            .padding()
            .background(Solarized.base2)
        }
    }

    private func messageRow(_ message: ChatMessage) -> some View {
        Group {
            if message.isSystem {
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
        guard !draft.isEmpty else { return }
        client.send(text: draft)
        draft = ""
    }
}
