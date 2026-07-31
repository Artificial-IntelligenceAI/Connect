package com.messagingapp.connect.ui

import android.content.Context
import android.os.Handler
import android.os.Looper
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import uniffi.messaging_core.ChatMessage as CoreChatMessage
import uniffi.messaging_core.ConnectClient
import uniffi.messaging_core.ConnectClientListener
import uniffi.messaging_core.ConnectionState as CoreConnectionState
import uniffi.messaging_core.Conversation as CoreConversation
import uniffi.messaging_core.GroupSummary
import uniffi.messaging_core.KnownPeer

sealed class Conversation {
    object System : Conversation()
    data class Direct(val peerIdentityKey: String) : Conversation()
    data class Group(val groupId: String, val groupName: String) : Conversation()
}

data class ChatMessage(val id: Long, val from: String, val text: String, val conversation: Conversation)

sealed class ConnectionState {
    object Disconnected : ConnectionState()
    object Connecting : ConnectionState()
    object Connected : ConnectionState()
    data class Reconnecting(val attempt: UInt) : ConnectionState()
    data class Failed(val reason: String) : ConnectionState()
}

/**
 * Thin wrapper around the Rust ConnectClient: implements the UniFFI
 * callback interface and republishes events as Compose state. Mirrors
 * shared/ConnectKit/Sources/ConnectKit/NetworkClient.swift.
 */
class NetworkClient(context: Context) {
    var state: ConnectionState by mutableStateOf(ConnectionState.Disconnected)
        private set
    val messages = mutableStateListOf<ChatMessage>()
    var knownPeers: List<KnownPeer> by mutableStateOf(emptyList())
        private set
    var groups: List<GroupSummary> by mutableStateOf(emptyList())
        private set

    // Context.filesDir is already app-private/sandboxed by Android, same
    // role as the app support directory on Apple platforms.
    private val client = ConnectClient(context.filesDir.absolutePath)
    private val mainHandler = Handler(Looper.getMainLooper())
    private var nextMessageId = 0L
    private val appContext = context.applicationContext

    fun connect(host: String, port: Int, displayName: String) {
        if (port !in 0..65535) {
            state = ConnectionState.Failed("Invalid port")
            return
        }
        client.connect(host, port.toUShort(), displayName, Listener())
    }

    fun sendDirectMessage(peerIdentityKey: String, text: String) {
        client.sendDirectMessage(peerIdentityKey, text)
    }

    /** Creates a group with [memberPeerIds] (currently-online peers only)
     * and invites each of them. Returns the new group's id, or `null` if
     * not connected or none of the member ids resolved to a known peer. */
    fun createGroup(name: String, memberPeerIds: List<String>): String? {
        return client.createGroup(name, memberPeerIds)
    }

    fun sendGroupMessage(groupId: String, text: String) {
        client.sendGroupMessage(groupId, text)
    }

    fun disconnect() {
        client.disconnect()
        state = ConnectionState.Disconnected
    }

    private inner class Listener : ConnectClientListener {
        override fun onStateChanged(state: CoreConnectionState) {
            mainHandler.post {
                this@NetworkClient.state = when (state) {
                    is CoreConnectionState.Disconnected -> ConnectionState.Disconnected
                    is CoreConnectionState.Connecting -> ConnectionState.Connecting
                    is CoreConnectionState.Connected -> ConnectionState.Connected
                    is CoreConnectionState.Reconnecting -> ConnectionState.Reconnecting(state.attempt)
                    is CoreConnectionState.Failed -> ConnectionState.Failed(state.reason)
                }
            }
        }

        override fun onMessage(message: CoreChatMessage) {
            mainHandler.post {
                val conversation = when (val c = message.conversation) {
                    is CoreConversation.System -> Conversation.System
                    is CoreConversation.Direct -> Conversation.Direct(c.peerIdentityKey)
                    is CoreConversation.Group -> Conversation.Group(c.groupId, c.groupName)
                }
                messages.add(
                    ChatMessage(nextMessageId++, message.from, message.text, conversation)
                )

                // Any message (a chat message or a system notice like "X
                // joined" or "Added to group") is a reasonable cue that the
                // contacts/group lists might have changed -- there's no
                // finer-grained push signal for this yet, and list sizes
                // are small enough that refreshing every time is cheap.
                knownPeers = client.listKnownPeers()
                groups = client.listGroups()

                // Only notify for actual chat content, and only while the
                // app is backgrounded -- no point interrupting someone
                // already looking at the conversation.
                if (!AppForegroundTracker.isForeground) {
                    when (conversation) {
                        is Conversation.Direct -> Notifications.notify(
                            appContext, "direct:${conversation.peerIdentityKey}", message.from, message.text
                        )
                        is Conversation.Group -> Notifications.notify(
                            appContext, "group:${conversation.groupId}", conversation.groupName, "${message.from}: ${message.text}"
                        )
                        Conversation.System -> {}
                    }
                }
            }
        }
    }
}
