package com.agentzero.client.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.Person
import androidx.compose.material.icons.filled.SmartToy
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.unit.dp
import com.agentzero.client.AppContainer
import com.agentzero.client.data.model.ChatMessage
import com.agentzero.client.data.model.ChatRole
import com.agentzero.client.data.model.ServerConfig
import com.agentzero.client.data.model.WsMessage
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.UUID

private const val EMPTY_DONE_FALLBACK =
    "Tool execution completed, but no final response text was returned."

@Composable
fun AgentChatScreen(config: ServerConfig, container: AppContainer) {
    val messages = remember { mutableStateListOf<ChatMessage>() }
    var input by remember { mutableStateOf("") }
    var connected by remember { mutableStateOf(false) }
    var typing by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var pendingContent by remember { mutableStateOf("") }
    val listState = rememberLazyListState()
    val wsClient = remember { container.chatWebSocketClient }
    val scope = rememberCoroutineScope()

    DisposableEffect(config) {
        val job = scope.launch {
            val existingSession = runCatching {
                container.settingsRepository.wsSessionId.first()
            }.getOrNull()

            val sessionId = existingSession?.takeIf { it.matches(SESSION_ID_REGEX) }
                ?: wsClient.newSessionId().also { newId ->
                    container.settingsRepository.saveWsSessionId(newId)
                    container.sessionIdCache = newId
                }
            container.sessionIdCache = sessionId

            wsClient.connect(config).collectLatest { event ->
                when (event) {
                    com.agentzero.client.data.ChatWebSocketClient.ChatWsEvent.Connected -> {
                        connected = true
                        error = null
                    }
                    com.agentzero.client.data.ChatWebSocketClient.ChatWsEvent.Disconnected ->
                        connected = false
                    is com.agentzero.client.data.ChatWebSocketClient.ChatWsEvent.Error -> {
                        connected = false
                        error = event.message
                    }
                    is com.agentzero.client.data.ChatWebSocketClient.ChatWsEvent.Message ->
                        handleWsMessage(
                            msg = event.message,
                            messages = messages,
                            onTyping = { typing = it },
                            setPending = { pendingContent = it },
                            getPending = { pendingContent },
                        )
                }
            }
        }

        onDispose { job.cancel() }
    }

    LaunchedEffect(messages.size, typing) {
        if (messages.isNotEmpty() || typing) {
            listState.animateScrollToItem(maxOf(0, messages.size - 1 + if (typing) 1 else 0))
        }
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .imePadding(),
    ) {
        error?.let {
            Text(
                it,
                color = MaterialTheme.colorScheme.error,
                modifier = Modifier
                    .fillMaxWidth()
                    .background(MaterialTheme.colorScheme.errorContainer)
                    .padding(8.dp),
            )
        }

        LazyColumn(
            state = listState,
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth()
                .padding(horizontal = 12.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            if (messages.isEmpty()) {
                item {
                    Column(
                        Modifier
                            .fillMaxWidth()
                            .padding(top = 48.dp),
                        horizontalAlignment = Alignment.CenterHorizontally,
                    ) {
                        Icon(Icons.Default.SmartToy, null, modifier = Modifier.size(48.dp))
                        Text("AgentZero", style = MaterialTheme.typography.titleLarge)
                        Text("Send a message to start the conversation")
                    }
                }
            }
            items(messages, key = { it.id }) { msg -> ChatBubble(msg) }
            if (typing) {
                item { TypingIndicator() }
            }
        }

        Surface(shadowElevation = 8.dp) {
            Column(Modifier.padding(12.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    OutlinedTextField(
                        value = input,
                        onValueChange = { input = it },
                        modifier = Modifier.weight(1f),
                        placeholder = { Text(if (connected) "Type a message..." else "Connecting...") },
                        enabled = connected,
                        singleLine = true,
                    )
                    IconButton(
                        onClick = {
                            val trimmed = input.trim()
                            if (trimmed.isEmpty() || !connected) return@IconButton
                            messages.add(
                                ChatMessage(
                                    id = UUID.randomUUID().toString(),
                                    role = ChatRole.User,
                                    content = trimmed,
                                ),
                            )
                            pendingContent = ""
                            typing = true
                            if (!wsClient.sendMessage(trimmed)) {
                                error = "Failed to send message."
                                typing = false
                            }
                            input = ""
                        },
                        enabled = connected && input.isNotBlank(),
                    ) {
                        Icon(Icons.AutoMirrored.Filled.Send, contentDescription = "Send")
                    }
                }
                ConnectionStatus(connected)
            }
        }
    }
}

@Composable
private fun ConnectionStatus(connected: Boolean) {
    Row(
        Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.Center,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            Modifier
                .size(8.dp)
                .clip(CircleShape)
                .background(
                    if (connected) MaterialTheme.colorScheme.primary
                    else MaterialTheme.colorScheme.error,
                ),
        )
        Spacer(Modifier.size(6.dp))
        Text(
            if (connected) "Connected" else "Disconnected",
            style = MaterialTheme.typography.labelSmall,
        )
    }
}

private fun handleWsMessage(
    msg: WsMessage,
    messages: MutableList<ChatMessage>,
    onTyping: (Boolean) -> Unit,
    setPending: (String) -> Unit,
    getPending: () -> String,
) {
    when (msg.type) {
        "history" -> {
            messages.clear()
            msg.messages.orEmpty()
                .filter { it.content.isNotBlank() }
                .forEach {
                    messages.add(
                        ChatMessage(
                            id = UUID.randomUUID().toString(),
                            role = if (it.role == "user") ChatRole.User else ChatRole.Agent,
                            content = it.content.trim(),
                        ),
                    )
                }
            setPending("")
            onTyping(false)
        }
        "chunk" -> {
            onTyping(true)
            setPending(getPending() + (msg.content ?: ""))
        }
        "message", "done" -> {
            val content = (msg.fullResponse ?: msg.content ?: getPending()).trim()
            messages.add(
                ChatMessage(
                    id = UUID.randomUUID().toString(),
                    role = ChatRole.Agent,
                    content = content.ifBlank { EMPTY_DONE_FALLBACK },
                ),
            )
            setPending("")
            onTyping(false)
        }
        "tool_call" -> {
            messages.add(
                ChatMessage(
                    id = UUID.randomUUID().toString(),
                    role = ChatRole.Agent,
                    content = "[Tool Call] ${msg.name ?: "unknown"}(${msg.args})",
                ),
            )
        }
        "tool_result" -> {
            messages.add(
                ChatMessage(
                    id = UUID.randomUUID().toString(),
                    role = ChatRole.Agent,
                    content = "[Tool Result] ${msg.output ?: ""}",
                ),
            )
        }
        "error" -> {
            messages.add(
                ChatMessage(
                    id = UUID.randomUUID().toString(),
                    role = ChatRole.Agent,
                    content = "[Error] ${msg.message ?: "Unknown error"}",
                ),
            )
            setPending("")
            onTyping(false)
        }
    }
}

@Composable
private fun ChatBubble(message: ChatMessage) {
    val isUser = message.role == ChatRole.User
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = if (isUser) Arrangement.End else Arrangement.Start,
    ) {
        if (!isUser) {
            Icon(Icons.Default.SmartToy, null, modifier = Modifier.size(24.dp))
            Spacer(Modifier.size(8.dp))
        }
        Surface(
            shape = RoundedCornerShape(12.dp),
            color = if (isUser) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.surfaceVariant,
            modifier = Modifier.widthIn(max = 300.dp),
        ) {
            Column(Modifier.padding(12.dp)) {
                Text(
                    message.content,
                    color = if (isUser) MaterialTheme.colorScheme.onPrimary else MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(
                    SimpleDateFormat("HH:mm", Locale.getDefault()).format(Date(message.timestampMillis)),
                    style = MaterialTheme.typography.labelSmall,
                    color = if (isUser) MaterialTheme.colorScheme.onPrimary.copy(alpha = 0.7f)
                    else MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.7f),
                )
            }
        }
        if (isUser) {
            Spacer(Modifier.size(8.dp))
            Icon(Icons.Default.Person, null, modifier = Modifier.size(24.dp))
        }
    }
}

@Composable
private fun TypingIndicator() {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Icon(Icons.Default.SmartToy, null, modifier = Modifier.size(24.dp))
        Spacer(Modifier.size(8.dp))
        Surface(shape = RoundedCornerShape(12.dp), color = MaterialTheme.colorScheme.surfaceVariant) {
            Text("Typing...", modifier = Modifier.padding(12.dp))
        }
    }
}

private val SESSION_ID_REGEX = Regex("^[A-Za-z0-9_-]{1,128}$")
