import Foundation

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

@MainActor
final class NetworkClient: ObservableObject {
    @Published var messages: [ChatMessage] = []
    @Published var state: ConnectionState = .disconnected

    private var task: URLSessionWebSocketTask?

    func connect(host: String, port: Int, displayName: String) {
        guard let url = URL(string: "ws://\(host):\(port)/ws") else {
            state = .failed("Invalid address")
            return
        }

        state = .connecting
        let session = URLSession(configuration: .default)
        let task = session.webSocketTask(with: url)
        self.task = task
        task.resume()
        state = .connected

        sendClientEvent(["type": "join", "display_name": displayName])
        listen()
    }

    func send(text: String) {
        sendClientEvent(["type": "message", "text": text])
    }

    func disconnect() {
        task?.cancel(with: .goingAway, reason: nil)
        task = nil
        state = .disconnected
    }

    private func sendClientEvent(_ payload: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: payload),
              let text = String(data: data, encoding: .utf8) else { return }
        task?.send(.string(text)) { [weak self] error in
            if let error {
                Task { @MainActor in
                    self?.state = .failed(error.localizedDescription)
                }
            }
        }
    }

    private func listen() {
        task?.receive { [weak self] result in
            guard let self else { return }
            Task { @MainActor in
                switch result {
                case .failure(let error):
                    self.state = .failed(error.localizedDescription)
                case .success(let message):
                    if case .string(let text) = message {
                        self.handleIncoming(text)
                    }
                    self.listen()
                }
            }
        }
    }

    private func handleIncoming(_ text: String) {
        guard let data = text.data(using: .utf8),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let type = json["type"] as? String else { return }

        switch type {
        case "message":
            let from = json["from"] as? String ?? "unknown"
            let text = json["text"] as? String ?? ""
            messages.append(ChatMessage(from: from, text: text, isSystem: false))
        case "system_notice":
            let text = json["text"] as? String ?? ""
            messages.append(ChatMessage(from: "", text: text, isSystem: true))
        default:
            break
        }
    }
}
