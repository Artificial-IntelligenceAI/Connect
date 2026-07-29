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

data class ChatMessage(val id: Long, val from: String, val text: String, val isSystem: Boolean)

sealed class ConnectionState {
    object Disconnected : ConnectionState()
    object Connecting : ConnectionState()
    object Connected : ConnectionState()
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

    // Context.filesDir is already app-private/sandboxed by Android, same
    // role as the app support directory on Apple platforms.
    private val client = ConnectClient(context.filesDir.absolutePath)
    private val mainHandler = Handler(Looper.getMainLooper())
    private var nextMessageId = 0L

    fun connect(host: String, port: Int, displayName: String) {
        if (port !in 0..65535) {
            state = ConnectionState.Failed("Invalid port")
            return
        }
        client.connect(host, port.toUShort(), displayName, Listener())
    }

    fun send(text: String) {
        client.send(text)
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
                    is CoreConnectionState.Failed -> ConnectionState.Failed(state.reason)
                }
            }
        }

        override fun onMessage(message: CoreChatMessage) {
            mainHandler.post {
                messages.add(
                    ChatMessage(nextMessageId++, message.from, message.text, message.isSystem)
                )
            }
        }
    }
}
