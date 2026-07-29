package com.messagingapp.connect.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp

@Composable
fun MainScreen(client: NetworkClient? = null) {
    val context = LocalContext.current
    val client = client ?: remember { NetworkClient(context) }
    Column(
        modifier = Modifier
            .fillMaxSize()
            .background(Solarized.base3),
        horizontalAlignment = Alignment.CenterHorizontally
    ) {
        when (client.state) {
            ConnectionState.Connected, is ConnectionState.Reconnecting -> ChatScreen(client)
            else -> ConnectScreen(client)
        }
    }
}

@Composable
private fun ConnectScreen(client: NetworkClient) {
    var host by remember { mutableStateOf("127.0.0.1") }
    var port by remember { mutableStateOf("7878") }
    var displayName by remember { mutableStateOf("") }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(32.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally
    ) {
        Text(
            "Connect to a LAN server",
            color = Solarized.base01,
            style = MaterialTheme.typography.titleLarge
        )
        Spacer(Modifier.height(12.dp))

        ThemedField(host, { host = it }, "Server address")
        Spacer(Modifier.height(12.dp))
        ThemedField(port, { port = it }, "Port")
        Spacer(Modifier.height(12.dp))
        ThemedField(displayName, { displayName = it }, "Display name")

        val failedReason = (client.state as? ConnectionState.Failed)?.reason
        if (failedReason != null) {
            Spacer(Modifier.height(8.dp))
            Text(failedReason, color = Solarized.red, style = MaterialTheme.typography.bodySmall)
        }

        Spacer(Modifier.height(12.dp))
        Button(
            onClick = {
                val portNumber = port.toIntOrNull()
                if (portNumber != null && displayName.isNotEmpty()) {
                    client.connect(host, portNumber, displayName)
                }
            },
            enabled = client.state != ConnectionState.Connecting,
            colors = ButtonDefaults.buttonColors(containerColor = Solarized.blue),
            shape = RoundedCornerShape(50)
        ) {
            Text("Connect", fontWeight = FontWeight.Bold)
        }
    }
}

@Composable
private fun ChatScreen(client: NetworkClient) {
    var draft by remember { mutableStateOf("") }
    val listState = rememberLazyListState()

    LaunchedEffect(client.messages.size) {
        if (client.messages.isNotEmpty()) {
            // Not animateScrollToItem: an animated scroll racing the IME's
            // window-resize animation can land mid-flight, leaving the list
            // scrolled past its content until the next recomposition.
            listState.scrollToItem(client.messages.size - 1)
        }
    }

    Column(modifier = Modifier.fillMaxSize()) {
        val reconnecting = client.state as? ConnectionState.Reconnecting
        if (reconnecting != null) {
            Text(
                "Reconnecting… (attempt ${reconnecting.attempt})",
                color = Solarized.yellow,
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier
                    .fillMaxWidth()
                    .background(Solarized.base2)
                    .padding(6.dp)
            )
        }
        LazyColumn(
            state = listState,
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth()
                .background(Solarized.base3)
                .padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp)
        ) {
            items(client.messages) { message ->
                if (message.conversation == Conversation.System) {
                    Text(
                        message.text,
                        color = Solarized.base1,
                        style = MaterialTheme.typography.bodySmall
                    )
                } else {
                    Column {
                        Text(
                            message.from,
                            color = Solarized.base01,
                            fontWeight = FontWeight.Bold,
                            style = MaterialTheme.typography.bodySmall
                        )
                        Text(message.text, color = Solarized.base00)
                    }
                }
            }
        }

        HorizontalDivider(color = Solarized.base1)

        // Disabled until the chat-list GUI exists: sending now needs a
        // peer or group to target, which this single free-text composer
        // has no way to express.
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .background(Solarized.base2)
                .padding(16.dp),
            verticalAlignment = Alignment.CenterVertically
        ) {
            OutlinedTextField(
                value = draft,
                onValueChange = { draft = it },
                modifier = Modifier.weight(1f),
                placeholder = { Text("Message") },
                colors = OutlinedTextFieldDefaults.colors(
                    unfocusedContainerColor = Solarized.base2,
                    focusedContainerColor = Solarized.base2,
                    unfocusedTextColor = Solarized.base00,
                    focusedTextColor = Solarized.base00,
                    unfocusedBorderColor = Solarized.base1,
                    focusedBorderColor = Solarized.blue
                )
            )
            Spacer(Modifier.width(8.dp))
            Button(
                onClick = {},
                enabled = false,
                colors = ButtonDefaults.buttonColors(containerColor = Solarized.blue)
            ) {
                Text("Send")
            }
        }
    }
}

@Composable
private fun ThemedField(value: String, onValueChange: (String) -> Unit, placeholder: String) {
    OutlinedTextField(
        value = value,
        onValueChange = onValueChange,
        placeholder = { Text(placeholder) },
        modifier = Modifier.widthIn(min = 280.dp),
        colors = OutlinedTextFieldDefaults.colors(
            unfocusedContainerColor = Solarized.base2,
            focusedContainerColor = Solarized.base2,
            unfocusedTextColor = Solarized.base00,
            focusedTextColor = Solarized.base00,
            unfocusedBorderColor = Solarized.base1,
            focusedBorderColor = Solarized.blue
        )
    )
}
