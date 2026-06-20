package com.agentzero.client.ui.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.AssistChip
import androidx.compose.material3.Card
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import com.agentzero.client.AppContainer
import com.agentzero.client.data.model.LogEntry
import com.agentzero.client.data.model.ServerConfig
import com.agentzero.client.data.model.SseEvent
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.launch
import kotlinx.serialization.json.Json
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.UUID

private const val MAX_LOG_ENTRIES = 500

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun MissionControlScreen(config: ServerConfig, container: AppContainer) {
    val entries = remember { mutableStateListOf<LogEntry>() }
    var connected by remember { mutableStateOf(false) }
    var paused by remember { mutableStateOf(false) }
    var autoScroll by remember { mutableStateOf(true) }
    var typeFilters by remember { mutableStateOf(setOf<String>()) }
    var selectedEntry by remember { mutableStateOf<LogEntry?>(null) }
    val listState = rememberLazyListState()
    val scope = rememberCoroutineScope()
    val json = remember { Json { prettyPrint = true } }

    DisposableEffect(config) {
        var job: Job? = null
        job = scope.launch {
            container.sseEventClient.connect(config).collectLatest { event ->
                when (event) {
                    com.agentzero.client.data.SseEventClient.SseConnectionEvent.Connected ->
                        connected = true
                    com.agentzero.client.data.SseEventClient.SseConnectionEvent.Disconnected ->
                        connected = false
                    is com.agentzero.client.data.SseEventClient.SseConnectionEvent.Error ->
                        connected = false
                    is com.agentzero.client.data.SseEventClient.SseConnectionEvent.Event -> {
                        if (!paused) {
                            entries.add(LogEntry(id = UUID.randomUUID().toString(), event = event.event))
                            while (entries.size > MAX_LOG_ENTRIES) entries.removeAt(0)
                        }
                    }
                }
            }
        }
        onDispose { job.cancel() }
    }

    LaunchedEffect(listState) {
        snapshotFlow {
            val layoutInfo = listState.layoutInfo
            val lastVisible = layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0
            lastVisible >= layoutInfo.totalItemsCount - 2
        }.distinctUntilChanged().collect { atBottom ->
            autoScroll = atBottom
        }
    }

    val filtered = if (typeFilters.isEmpty()) {
        entries
    } else {
        entries.filter { typeFilters.contains(it.event.type) }
    }

    val allTypes = entries.map { it.event.type }.distinct().sorted()

    LaunchedEffect(filtered.size, autoScroll) {
        if (autoScroll && filtered.isNotEmpty()) {
            listState.animateScrollToItem(filtered.lastIndex)
        }
    }

    Column(Modifier.fillMaxSize()) {
        Row(
            Modifier
                .fillMaxWidth()
                .padding(horizontal = 12.dp, vertical = 8.dp),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column {
                Text("Mission Control", style = MaterialTheme.typography.titleMedium)
                Text(
                    "${if (connected) "Connected" else "Disconnected"} · ${filtered.size} events",
                    style = MaterialTheme.typography.labelSmall,
                )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (!autoScroll) {
                    TextButton(onClick = {
                        scope.launch {
                            if (filtered.isNotEmpty()) listState.animateScrollToItem(filtered.lastIndex)
                            autoScroll = true
                        }
                    }) {
                        Icon(Icons.Default.KeyboardArrowDown, null)
                        Text("Bottom")
                    }
                }
                TextButton(onClick = { paused = !paused }) {
                    Icon(if (paused) Icons.Default.PlayArrow else Icons.Default.Pause, null)
                    Text(if (paused) "Resume" else "Pause")
                }
            }
        }

        if (allTypes.isNotEmpty()) {
            FlowRow(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 12.dp),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                allTypes.forEach { type ->
                    FilterChip(
                        selected = typeFilters.contains(type),
                        onClick = {
                            typeFilters = if (typeFilters.contains(type)) {
                                typeFilters - type
                            } else {
                                typeFilters + type
                            }
                        },
                        label = { Text(type) },
                    )
                }
                if (typeFilters.isNotEmpty()) {
                    AssistChip(onClick = { typeFilters = emptySet() }, label = { Text("Clear") })
                }
            }
        }

        LazyColumn(
            state = listState,
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth()
                .padding(12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            if (filtered.isEmpty()) {
                item {
                    Text(
                        if (paused) "Log streaming is paused." else "Waiting for events...",
                        modifier = Modifier.padding(top = 32.dp),
                    )
                }
            }
            items(filtered, key = { it.id }) { entry ->
                Card(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable { selectedEntry = entry },
                ) {
                    Column(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(12.dp),
                        verticalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        Text(
                            formatTimestamp(entry.event.timestamp),
                            style = MaterialTheme.typography.labelSmall,
                            fontFamily = FontFamily.Monospace,
                        )
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                            verticalAlignment = Alignment.Top,
                        ) {
                            Surface(
                                shape = RoundedCornerShape(4.dp),
                                color = MaterialTheme.colorScheme.secondaryContainer,
                            ) {
                                Text(
                                    entry.event.type,
                                    modifier = Modifier.padding(horizontal = 6.dp, vertical = 2.dp),
                                    style = MaterialTheme.typography.labelSmall,
                                    color = MaterialTheme.colorScheme.onSecondaryContainer,
                                )
                            }
                            val detail = formatEventDetail(entry.event)
                            if (detail.isNotBlank()) {
                                Text(
                                    detail,
                                    style = MaterialTheme.typography.bodySmall,
                                    modifier = Modifier.weight(1f),
                                )
                            }
                        }
                    }
                }
            }
        }
    }

    selectedEntry?.let { entry ->
        AlertDialog(
            onDismissRequest = { selectedEntry = null },
            confirmButton = {
                TextButton(onClick = { selectedEntry = null }) { Text("Close") }
            },
            title = { Text("Event Details") },
            text = {
                LazyColumn {
                    item {
                        Text("Type: ${entry.event.type}")
                        Text("Timestamp: ${formatTimestamp(entry.event.timestamp)}")
                        Text(
                            json.encodeToString(SseEvent.serializer(), entry.event),
                            fontFamily = FontFamily.Monospace,
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }
            },
        )
    }
}

private fun formatTimestamp(ts: String?): String {
    val formatter = SimpleDateFormat.getDateTimeInstance(
        SimpleDateFormat.SHORT,
        SimpleDateFormat.MEDIUM,
        Locale.getDefault(),
    )
    if (ts.isNullOrBlank()) {
        return formatter.format(Date())
    }
    return runCatching {
        val instant = java.time.Instant.parse(ts)
        formatter.format(Date.from(instant))
    }.getOrElse { ts }
}

private fun formatEventDetail(event: SseEvent): String = when (event.type) {
    "turn_complete" -> "Agent completed turn"
    "channel_message" -> {
        val dir = if (event.direction == "inbound") "Received" else "Sent"
        "$dir on ${event.channel ?: "unknown"}"
    }
    "webhook_auth_failure" ->
        "Auth failure on ${event.channel} (signature: ${event.signature}, bearer: ${event.bearer})"
    "heartbeat_tick" -> "Runtime heartbeat"
    else -> listOfNotNull(
        event.message?.takeIf { it.isNotBlank() },
        event.content?.takeIf { it.isNotBlank() },
        event.data?.takeIf { it.isNotBlank() },
    ).firstOrNull().orEmpty()
}
