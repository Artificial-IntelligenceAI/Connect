import SwiftUI

struct ContentView: View {
    @StateObject private var client = NetworkClient()

    @State private var host = "127.0.0.1"
    @State private var port = "7878"
    @State private var displayName = ""
    @State private var draft = ""

    var body: some View {
        Group {
            switch client.state {
            case .connected:
                chatView
            default:
                connectView
            }
        }
        .frame(minWidth: 420, minHeight: 500)
    }

    private var connectView: some View {
        VStack(spacing: 12) {
            Text("Connect to a LAN server")
                .font(.title2)

            TextField("Server address", text: $host)
            TextField("Port", text: $port)
            TextField("Display name", text: $displayName)

            if case .failed(let reason) = client.state {
                Text(reason).foregroundStyle(.red).font(.caption)
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
                .onChange(of: client.messages.count) { _ in
                    if let last = client.messages.last {
                        proxy.scrollTo(last.id, anchor: .bottom)
                    }
                }
            }

            Divider()

            HStack {
                TextField("Message", text: $draft)
                    .onSubmit(sendDraft)
                Button("Send", action: sendDraft)
                    .disabled(draft.isEmpty)
            }
            .padding()
        }
    }

    private func messageRow(_ message: ChatMessage) -> some View {
        Group {
            if message.isSystem {
                Text(message.text)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else {
                VStack(alignment: .leading, spacing: 2) {
                    Text(message.from).font(.caption).bold()
                    Text(message.text)
                }
            }
        }
    }

    private func sendDraft() {
        guard !draft.isEmpty else { return }
        client.send(text: draft)
        draft = ""
    }
}
