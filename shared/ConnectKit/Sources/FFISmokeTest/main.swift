// Standalone smoke test for the MessagingCore FFI layer: connects to a
// locally running messaging-server and prints everything the Rust core
// reports back (fingerprint, TOFU notices, join/leave). Not a UI test --
// exercises the Rust<->Swift boundary directly. Run two instances with
// different display names to verify they discover each other.
//
// Doesn't exercise sending a 1:1/group message: that needs a live peer_id,
// and there's no query API yet for "who's currently online" over this
// listener interface (ConnectClientListener only reports ChatMessage/
// ConnectionState) -- that's part of the deferred chat-list GUI work, not
// this smoke test. `core/src/client.rs`'s own test suite covers DM/group
// send+receive directly instead.
//
// Identity is persisted per dataDirTag (defaults to displayName), under
// a temp directory -- re-running with the same tag reuses the same
// identity (test persistence), a different tag gets a fresh one (test
// the key-changed warning by reusing a displayName with a different tag).
//
// Usage: swift run FFISmokeTest [displayName] [host] [port] [dataDirTag]

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
        print("[message] from=\"\(message.from)\" text=\"\(message.text)\" conversation=\(message.conversation)")
    }
}

let args = CommandLine.arguments
let displayName = args.count > 1 ? args[1] : "FFISmokeTest"
let host = args.count > 2 ? args[2] : "127.0.0.1"
let port = args.count > 3 ? UInt16(args[3]) ?? 7878 : 7878
let dataDirTag = args.count > 4 ? args[4] : displayName

let dataDir = FileManager.default.temporaryDirectory
    .appendingPathComponent("ConnectSmokeTest", isDirectory: true)
    .appendingPathComponent(dataDirTag, isDirectory: true)
try? FileManager.default.createDirectory(at: dataDir, withIntermediateDirectories: true)

let listener = TestListener()
let client = ConnectClient(dataDir: dataDir.path)

print("Connecting to \(host):\(port) as \(displayName)...")
client.connect(host: host, port: port, displayName: displayName, listener: listener)

if listener.connectedSemaphore.wait(timeout: .now() + 5) == .timedOut {
    print("TIMEOUT waiting to connect")
    exit(1)
}

// Stay alive so any other instance connecting concurrently shows up as a
// PeerJoined/TOFU notice above.
Thread.sleep(forTimeInterval: 4.0)
client.disconnect()
print("done")
