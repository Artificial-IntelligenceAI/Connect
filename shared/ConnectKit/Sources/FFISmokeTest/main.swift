// Standalone smoke test for the MessagingCore FFI layer: connects to a
// locally running messaging-server, sends one message, and prints
// everything the Rust core reports back. Not a UI test -- exercises the
// Rust<->Swift boundary (and, now, the E2EE layer) directly. Run two
// instances with different display names to verify real encrypted
// peer-to-peer delivery, not just a loopback.
//
// Usage: swift run FFISmokeTest [displayName] [host] [port]

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
let displayName = args.count > 1 ? args[1] : "FFISmokeTest"
let host = args.count > 2 ? args[2] : "127.0.0.1"
let port = args.count > 3 ? UInt16(args[3]) ?? 7878 : 7878

let listener = TestListener()
let client = ConnectClient()

print("Connecting to \(host):\(port) as \(displayName)...")
client.connect(host: host, port: port, displayName: displayName, listener: listener)

if listener.connectedSemaphore.wait(timeout: .now() + 5) == .timedOut {
    print("TIMEOUT waiting to connect")
    exit(1)
}

// Give any other already-connected peer a moment to key-exchange with us
// before we send, and stay alive afterwards so we can receive from others too.
Thread.sleep(forTimeInterval: 1.5)
print("Sending test message...")
client.send(text: "Hello from \(displayName)")

Thread.sleep(forTimeInterval: 4.0)
client.disconnect()
print("done")
