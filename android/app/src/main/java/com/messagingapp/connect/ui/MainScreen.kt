package com.messagingapp.connect.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Checkbox
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExposedDropdownMenuBox
import androidx.compose.material3.ExposedDropdownMenuDefaults
import androidx.compose.material3.MenuAnchorType
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import uniffi.messaging_core.KnownPeer

enum class ChatFilter(val label: String) {
    ALL("All"), DIRECT("DM"), GROUP("GC")
}

sealed class SelectedConversation {
    data class Direct(val peerIdentityKey: String, val displayName: String) : SelectedConversation()
    data class Group(val groupId: String, val groupName: String) : SelectedConversation()

    val title: String
        get() = when (this) {
            is Direct -> displayName
            is Group -> groupName
        }
}

private data class ConversationEntry(
    val id: String,
    val title: String,
    val subtitle: String,
    val target: SelectedConversation
)

/// Lets a tap anywhere outside a focused field dismiss the keyboard --
/// Compose has no built-in "tap elsewhere to dismiss" behavior, unlike a
/// plain scroll gesture inside a text field's own container.
@Composable
private fun Modifier.dismissKeyboardOnTap(): Modifier {
    val focusManager = LocalFocusManager.current
    val keyboardController = LocalSoftwareKeyboardController.current
    return this.pointerInput(Unit) {
        detectTapGestures(onTap = {
            focusManager.clearFocus()
            keyboardController?.hide()
        })
    }
}

@Composable
fun MainScreen(client: NetworkClient? = null) {
    val context = LocalContext.current
    val client = client ?: remember { NetworkClient(context) }
    // Eagerly load persisted settings here, before any descendant composable
    // (including this Column's own `.background` below) reads `Solarized.current` --
    // otherwise the first frame renders with the default theme and only the
    // children that happen to trigger AppSettings.getInstance() (e.g. ThemedField)
    // pick up the persisted value, producing a mismatched partial-theme flash.
    remember { AppSettings.getInstance(context) }
    var selectedConversation by remember { mutableStateOf<SelectedConversation?>(null) }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .background(Solarized.base3)
            .dismissKeyboardOnTap(),
        horizontalAlignment = Alignment.CenterHorizontally
    ) {
        when (client.state) {
            ConnectionState.Connected, is ConnectionState.Reconnecting -> {
                val selected = selectedConversation
                if (selected != null) {
                    ConversationScreen(client, selected, onBack = { selectedConversation = null })
                } else {
                    ChatListScreen(client, onOpen = { selectedConversation = it })
                }
            }
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
private fun ReconnectingBanner(attempt: UInt) {
    Text(
        "Reconnecting… (attempt $attempt)",
        color = Solarized.yellow,
        style = MaterialTheme.typography.bodySmall,
        modifier = Modifier
            .fillMaxWidth()
            .background(Solarized.base2)
            .padding(6.dp)
    )
}

@Composable
private fun ChatListScreen(client: NetworkClient, onOpen: (SelectedConversation) -> Unit) {
    var filter by remember { mutableStateOf(ChatFilter.ALL) }
    var searchText by remember { mutableStateOf("") }
    var showingCreateGroup by remember { mutableStateOf(false) }
    var showingSettings by remember { mutableStateOf(false) }

    Row(modifier = Modifier.fillMaxSize()) {
        FilterSidebar(filter, onSelect = { filter = it }, onSettingsClick = { showingSettings = true })

        Column(modifier = Modifier.weight(1f)) {
            if (client.state is ConnectionState.Reconnecting) {
                ReconnectingBanner((client.state as ConnectionState.Reconnecting).attempt)
            }

            Row(
                modifier = Modifier.fillMaxWidth().padding(12.dp),
                verticalAlignment = Alignment.CenterVertically
            ) {
                Text("Chats", style = MaterialTheme.typography.titleMedium, color = Solarized.base01)
                Spacer(Modifier.weight(1f))
                TextButton(onClick = { showingCreateGroup = true }) {
                    Text("+", color = Solarized.blue, style = MaterialTheme.typography.titleLarge)
                }
            }

            ThemedField(searchText, { searchText = it }, "Search", modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp))

            // Connection-lifecycle notices (fingerprint, join/leave, TOFU
            // warnings) are room-wide, not tied to any one conversation, so
            // they live here rather than inside a per-conversation view.
            val notices = client.messages.filter { it.conversation == Conversation.System }
            if (notices.isNotEmpty()) {
                LazyColumn(modifier = Modifier.fillMaxWidth().heightIn(max = 80.dp).padding(horizontal = 12.dp, vertical = 4.dp)) {
                    items(notices) { notice ->
                        Text(notice.text, color = Solarized.base1, style = MaterialTheme.typography.bodySmall)
                    }
                }
            }

            val entries = conversationEntries(client, filter, searchText)
            if (entries.isEmpty()) {
                Box(modifier = Modifier.fillMaxWidth().weight(1f), contentAlignment = Alignment.Center) {
                    Text(
                        if (searchText.isEmpty()) "No conversations yet" else "No matches",
                        color = Solarized.base1,
                        style = MaterialTheme.typography.bodySmall
                    )
                }
            } else {
                LazyColumn(modifier = Modifier.fillMaxWidth().weight(1f)) {
                    items(entries) { entry ->
                        Column(
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable { onOpen(entry.target) }
                                .padding(horizontal = 16.dp, vertical = 10.dp)
                        ) {
                            Text(entry.title, color = Solarized.base01)
                            Text(entry.subtitle, color = Solarized.base1, style = MaterialTheme.typography.bodySmall)
                        }
                        HorizontalDivider(color = Solarized.base2)
                    }
                }
            }
        }
    }

    if (showingCreateGroup) {
        CreateGroupDialog(
            onlinePeers = client.knownPeers.filter { it.peerId != null },
            onCreate = { name, memberPeerIds ->
                val groupId = client.createGroup(name, memberPeerIds)
                if (groupId != null) {
                    onOpen(SelectedConversation.Group(groupId, name))
                }
                showingCreateGroup = false
            },
            onCancel = { showingCreateGroup = false }
        )
    }

    if (showingSettings) {
        SettingsDialog(onDismiss = { showingSettings = false })
    }
}

@Composable
private fun FilterSidebar(selected: ChatFilter, onSelect: (ChatFilter) -> Unit, onSettingsClick: () -> Unit) {
    Column(
        modifier = Modifier
            .width(72.dp)
            .fillMaxSize()
            .background(Solarized.base2)
            .padding(8.dp),
        verticalArrangement = Arrangement.Center
    ) {
        for (option in ChatFilter.values()) {
            val isSelected = option == selected
            Text(
                option.label,
                color = if (isSelected) Solarized.blue else Solarized.base01,
                style = MaterialTheme.typography.labelLarge,
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable { onSelect(option) }
                    .background(if (isSelected) Solarized.blue.copy(alpha = 0.2f) else Solarized.base2, RoundedCornerShape(6.dp))
                    .padding(vertical = 10.dp),
                textAlign = androidx.compose.ui.text.style.TextAlign.Center
            )
            Spacer(Modifier.height(8.dp))
        }

        Text(
            "⚙",
            color = Solarized.base01,
            style = MaterialTheme.typography.titleLarge,
            modifier = Modifier
                .fillMaxWidth()
                .clickable { onSettingsClick() }
                .padding(vertical = 10.dp),
            textAlign = androidx.compose.ui.text.style.TextAlign.Center
        )
    }
}

private fun conversationEntries(client: NetworkClient, filter: ChatFilter, searchText: String): List<ConversationEntry> {
    val entries = mutableListOf<ConversationEntry>()
    if (filter != ChatFilter.GROUP) {
        for (peer in client.knownPeers) {
            entries.add(
                ConversationEntry(
                    id = "direct:${peer.identityKey}",
                    title = peer.displayName,
                    subtitle = if (peer.peerId != null) "online" else "offline",
                    target = SelectedConversation.Direct(peer.identityKey, peer.displayName)
                )
            )
        }
    }
    if (filter != ChatFilter.DIRECT) {
        for (group in client.groups) {
            entries.add(
                ConversationEntry(
                    id = "group:${group.groupId}",
                    title = group.name,
                    subtitle = "${group.memberCount} member" + if (group.memberCount == 1u) "" else "s",
                    target = SelectedConversation.Group(group.groupId, group.name)
                )
            )
        }
    }
    val query = searchText.trim()
    val filtered = if (query.isEmpty()) entries else entries.filter { it.title.contains(query, ignoreCase = true) }
    return filtered.sortedBy { it.title.lowercase() }
}

/** A minimal group-creation flow: name + pick from currently-online known
 * peers. Membership is fixed at creation (see `ConnectClient.createGroup`'s
 * v1 limitations) so there's deliberately no "invite offline" affordance. */
@Composable
private fun CreateGroupDialog(
    onlinePeers: List<KnownPeer>,
    onCreate: (String, List<String>) -> Unit,
    onCancel: () -> Unit
) {
    var name by remember { mutableStateOf("") }
    var selectedPeerIds by remember { mutableStateOf(setOf<String>()) }

    Dialog(onDismissRequest = onCancel) {
        Column(
            modifier = Modifier
                .background(Solarized.base3, RoundedCornerShape(8.dp))
                .padding(20.dp)
                .widthIn(min = 300.dp)
                .dismissKeyboardOnTap()
                .imePadding()
        ) {
            Text("New group chat", style = MaterialTheme.typography.titleMedium, color = Solarized.base01)
            Spacer(Modifier.height(12.dp))
            ThemedField(name, { name = it }, "Group name", modifier = Modifier.fillMaxWidth())
            Spacer(Modifier.height(12.dp))
            Text("Members (online now)", color = Solarized.base1, style = MaterialTheme.typography.bodySmall)

            if (onlinePeers.isEmpty()) {
                Text("No one else is online right now.", color = Solarized.base1, style = MaterialTheme.typography.bodySmall)
            } else {
                LazyColumn(modifier = Modifier.heightIn(max = 200.dp)) {
                    items(onlinePeers) { peer ->
                        val peerId = peer.peerId
                        if (peerId != null) {
                            Row(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .clickable {
                                        selectedPeerIds = if (selectedPeerIds.contains(peerId)) {
                                            selectedPeerIds - peerId
                                        } else {
                                            selectedPeerIds + peerId
                                        }
                                    }
                                    .padding(vertical = 6.dp),
                                verticalAlignment = Alignment.CenterVertically
                            ) {
                                Checkbox(checked = selectedPeerIds.contains(peerId), onCheckedChange = null)
                                Text(peer.displayName, color = Solarized.base01)
                            }
                        }
                    }
                }
            }

            Spacer(Modifier.height(16.dp))
            Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                TextButton(onClick = onCancel) { Text("Cancel") }
                Button(
                    onClick = { onCreate(name, selectedPeerIds.toList()) },
                    enabled = name.isNotEmpty() && selectedPeerIds.isNotEmpty(),
                    colors = ButtonDefaults.buttonColors(containerColor = Solarized.blue)
                ) {
                    Text("Create")
                }
            }
        }
    }
}

@Composable
private fun ConversationScreen(client: NetworkClient, selected: SelectedConversation, onBack: () -> Unit) {
    var draft by remember { mutableStateOf("") }
    val listState = rememberLazyListState()

    val conversationMessages = client.messages.filter { message ->
        when (val c = message.conversation) {
            is Conversation.Direct -> (selected as? SelectedConversation.Direct)?.peerIdentityKey == c.peerIdentityKey
            is Conversation.Group -> (selected as? SelectedConversation.Group)?.groupId == c.groupId
            Conversation.System -> false
        }
    }

    LaunchedEffect(conversationMessages.size) {
        if (conversationMessages.isNotEmpty()) {
            // Not animateScrollToItem: an animated scroll racing the IME's
            // window-resize animation can land mid-flight, leaving the list
            // scrolled past its content until the next recomposition.
            listState.scrollToItem(conversationMessages.size - 1)
        }
    }

    Column(modifier = Modifier.fillMaxSize()) {
        Row(
            modifier = Modifier.fillMaxWidth().background(Solarized.base2).padding(12.dp),
            verticalAlignment = Alignment.CenterVertically
        ) {
            TextButton(onClick = onBack) { Text("← Back", color = Solarized.blue) }
            Spacer(Modifier.weight(1f))
            Text(selected.title, color = Solarized.base01, style = MaterialTheme.typography.titleMedium)
            Spacer(Modifier.weight(1f))
            Spacer(Modifier.width(56.dp)) // balances the back button so the title stays centered
        }

        if (client.state is ConnectionState.Reconnecting) {
            ReconnectingBanner((client.state as ConnectionState.Reconnecting).attempt)
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
            items(conversationMessages) { message ->
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

        HorizontalDivider(color = Solarized.base1)

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
                onClick = {
                    if (draft.isNotEmpty()) {
                        when (selected) {
                            is SelectedConversation.Direct -> client.sendDirectMessage(selected.peerIdentityKey, draft)
                            is SelectedConversation.Group -> client.sendGroupMessage(selected.groupId, draft)
                        }
                        draft = ""
                    }
                },
                enabled = draft.isNotEmpty(),
                colors = ButtonDefaults.buttonColors(containerColor = Solarized.blue)
            ) {
                Text("Send")
            }
        }
    }
}

@Composable
private fun ThemedField(
    value: String,
    onValueChange: (String) -> Unit,
    placeholder: String,
    modifier: Modifier = Modifier.widthIn(min = 280.dp)
) {
    val settings = AppSettings.getInstance(LocalContext.current)
    OutlinedTextField(
        value = value,
        onValueChange = onValueChange,
        placeholder = { Text(placeholder) },
        modifier = modifier,
        keyboardOptions = KeyboardOptions(autoCorrectEnabled = settings.autoCorrectEnabled),
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

/** Theme (dropdown) and Auto-Correct (toggle) -- the only two settings so
 * far. Reads/writes the shared [AppSettings] instance so changes apply
 * live and persist immediately. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SettingsDialog(onDismiss: () -> Unit) {
    val settings = AppSettings.getInstance(LocalContext.current)
    var themeMenuExpanded by remember { mutableStateOf(false) }

    Dialog(onDismissRequest = onDismiss) {
        Column(
            modifier = Modifier
                .background(Solarized.base3, RoundedCornerShape(8.dp))
                .padding(20.dp)
                .widthIn(min = 280.dp)
        ) {
            Text("Settings", style = MaterialTheme.typography.titleMedium, color = Solarized.base01)
            Spacer(Modifier.height(16.dp))

            Text("Theme", color = Solarized.base01, style = MaterialTheme.typography.bodySmall)
            Spacer(Modifier.height(4.dp))
            ExposedDropdownMenuBox(
                expanded = themeMenuExpanded,
                onExpandedChange = { themeMenuExpanded = it }
            ) {
                OutlinedTextField(
                    value = settings.theme.label,
                    onValueChange = {},
                    readOnly = true,
                    trailingIcon = { ExposedDropdownMenuDefaults.TrailingIcon(expanded = themeMenuExpanded) },
                    modifier = Modifier.menuAnchor(MenuAnchorType.PrimaryNotEditable).fillMaxWidth(),
                    colors = OutlinedTextFieldDefaults.colors(
                        unfocusedContainerColor = Solarized.base2,
                        focusedContainerColor = Solarized.base2,
                        unfocusedTextColor = Solarized.base00,
                        focusedTextColor = Solarized.base00,
                        unfocusedBorderColor = Solarized.base1,
                        focusedBorderColor = Solarized.blue
                    )
                )
                ExposedDropdownMenu(
                    expanded = themeMenuExpanded,
                    onDismissRequest = { themeMenuExpanded = false }
                ) {
                    AppTheme.entries.forEach { option ->
                        DropdownMenuItem(
                            text = { Text(option.label) },
                            onClick = {
                                settings.selectTheme(option)
                                themeMenuExpanded = false
                            }
                        )
                    }
                }
            }

            Spacer(Modifier.height(16.dp))

            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically
            ) {
                Text("Auto-Correct", color = Solarized.base01, modifier = Modifier.weight(1f))
                Switch(
                    checked = settings.autoCorrectEnabled,
                    onCheckedChange = { settings.setAutoCorrect(it) },
                    colors = SwitchDefaults.colors(checkedTrackColor = Solarized.blue)
                )
            }
        }
    }
}
