// Standalone smoke test for the MessagingCore FFI layer: connects to a
// locally running messaging-server and prints everything the Rust core
// reports back (fingerprint, TOFU notices, join/leave). Not a UI test --
// exercises the Rust<->Swift boundary directly. Run two instances with
// different display names to verify they discover each other.
//
// Optionally sends a DM: if sendToDisplayName/sendToText are given, polls
// listKnownPeers() for a peer with that display name (via TOFU discovery
// from its join notice) and sends it a direct message once found.
//
// Identity is persisted per dataDirTag (defaults to displayName), under
// a temp directory -- re-running with the same tag reuses the same
// identity (test persistence), a different tag gets a fresh one (test
// the key-changed warning by reusing a displayName with a different tag).
//
// Usage: swift run FFISmokeTest [displayName] [host] [port] [dataDirTag] [sendToDisplayName] [sendToText] [stayAliveSeconds]

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
// An empty string for either of these positional args means "not set" --
// lets a caller supply stayAliveSeconds (arg 8) without also being forced
// to trigger the send path.
let sendToDisplayName = (args.count > 5 && !args[5].isEmpty) ? args[5] : nil
let sendToText = (args.count > 6 && !args[6].isEmpty) ? args[6] : nil
let stayAliveSeconds = args.count > 7 ? Double(args[7]) ?? 4.0 : 4.0

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

if let sendToDisplayName, let sendToText {
    var target: KnownPeer?
    for _ in 0..<10 {
        target = client.listKnownPeers().first { $0.displayName == sendToDisplayName }
        if target != nil { break }
        Thread.sleep(forTimeInterval: 0.5)
    }
    guard let target else {
        print("TIMEOUT waiting for peer \"\(sendToDisplayName)\" to be known")
        exit(1)
    }
    print("Sending to \(sendToDisplayName) (\(target.peerId == nil ? "offline" : "online")): \(sendToText)")
    client.sendDirectMessage(peerIdentityKey: target.identityKey, text: sendToText)
    Thread.sleep(forTimeInterval: 1.0)
} else {
    // Stay alive so any other instance connecting concurrently shows up as
    // a PeerJoined/TOFU notice above.
    Thread.sleep(forTimeInterval: stayAliveSeconds)
}
client.disconnect()
print("done")
