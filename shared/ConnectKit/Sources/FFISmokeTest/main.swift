// Standalone smoke test for the MessagingCore FFI layer: connects to a
// locally running messaging-server, sends one message, and prints
// everything the Rust core reports back. Not a UI test -- exercises the
// Rust<->Swift boundary directly.
//
// Usage: swift run FFISmokeTest [host] [port]

import Foundation
import MessagingCore

final class TestListener: ConnectClientListener {
    let connectedSemaphore = DispatchSemaphore(value: 0)

    func onStateChanged(state: ConnectionState) {
        print("[state] \(state)")
        switch state {
        case .connected:
            connectedSemaphore.signal()
        case .failed(let reason):
            print("FAILED: \(reason)")
            exit(1)
        default:
            break
        }
    }

    func onMessage(message: ChatMessage) {
        print("[message] from=\"\(message.from)\" text=\"\(message.text)\" isSystem=\(message.isSystem)")
    }
}

let args = CommandLine.arguments
let host = args.count > 1 ? args[1] : "127.0.0.1"
let port = args.count > 2 ? UInt16(args[2]) ?? 7878 : 7878

let listener = TestListener()
let client = ConnectClient()

print("Connecting to \(host):\(port) as FFISmokeTest...")
client.connect(host: host, port: port, displayName: "FFISmokeTest", listener: listener)

if listener.connectedSemaphore.wait(timeout: .now() + 5) == .timedOut {
    print("TIMEOUT waiting to connect")
    exit(1)
}

print("Sending test message...")
client.send(text: "Hello from the Rust core via FFI")

Thread.sleep(forTimeInterval: 1.5)
client.disconnect()
print("done")
