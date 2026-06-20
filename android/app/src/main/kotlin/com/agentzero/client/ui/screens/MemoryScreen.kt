package com.agentzero.client.ui.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.agentzero.client.AppContainer
import com.agentzero.client.data.model.MemoryEntry
import com.agentzero.client.data.model.ServerConfig
import kotlinx.coroutines.launch
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

@Composable
fun MemoryScreen(config: ServerConfig, container: AppContainer) {
    var entries by remember { mutableStateOf<List<MemoryEntry>>(emptyList()) }
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    var search by remember { mutableStateOf("") }
    var categoryFilter by remember { mutableStateOf("") }
    var selected by remember { mutableStateOf<MemoryEntry?>(null) }
    var showAdd by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf<MemoryEntry?>(null) }
    val scope = rememberCoroutineScope()

    fun reload() {
        scope.launch {
            loading = true
            error = null
            runCatching {
                entries = container.gatewayClient.getMemory(
                    config,
                    query = search.takeIf { it.isNotBlank() },
                    category = categoryFilter.takeIf { it.isNotBlank() },
                )
            }.onFailure { error = it.message }
            loading = false
        }
    }

    LaunchedEffect(config) { reload() }

    Scaffold(
        floatingActionButton = {
            FloatingActionButton(onClick = { showAdd = true }) {
                Icon(Icons.Default.Add, contentDescription = "Add memory")
            }
        },
    ) { padding ->
        Column(
            Modifier
                .fillMaxSize()
                .padding(padding)
                .padding(12.dp),
        ) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(
                    value = search,
                    onValueChange = { search = it },
                    modifier = Modifier.weight(1f),
                    label = { Text("Search") },
                    singleLine = true,
                )
                IconButton(onClick = { reload() }) {
                    Icon(Icons.Default.Search, contentDescription = "Search")
                }
            }
            OutlinedTextField(
                value = categoryFilter,
                onValueChange = { categoryFilter = it },
                modifier = Modifier.fillMaxWidth(),
                label = { Text("Category filter") },
                singleLine = true,
            )

            error?.let {
                Text(it, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(top = 8.dp))
            }

            when {
                loading -> Column(
                    Modifier.fillMaxSize(),
                    verticalArrangement = Arrangement.Center,
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) { CircularProgressIndicator() }

                entries.isEmpty() -> Text("No memory entries found.", modifier = Modifier.padding(top = 24.dp))

                else -> LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    items(entries, key = { it.key }) { entry ->
                        Card(
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable { selected = entry },
                        ) {
                            Row(
                                Modifier
                                    .fillMaxWidth()
                                    .padding(12.dp),
                                horizontalArrangement = Arrangement.SpaceBetween,
                            ) {
                                Column(Modifier.weight(1f)) {
                                    Text(entry.key, style = MaterialTheme.typography.titleSmall)
                                    Text(entry.category, style = MaterialTheme.typography.labelSmall)
                                    Text(
                                        entry.content,
                                        maxLines = 2,
                                        overflow = TextOverflow.Ellipsis,
                                        style = MaterialTheme.typography.bodySmall,
                                    )
                                }
                                IconButton(onClick = { confirmDelete = entry }) {
                                    Icon(Icons.Default.Delete, contentDescription = "Delete")
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    selected?.let { entry ->
        AlertDialog(
            onDismissRequest = { selected = null },
            confirmButton = { TextButton(onClick = { selected = null }) { Text("Close") } },
            title = { Text(entry.key) },
            text = {
                Column {
                    Text("Category: ${entry.category}")
                    Text("Created: ${formatDate(entry.timestamp)}")
                    Text(entry.content)
                }
            },
        )
    }

    confirmDelete?.let { entry ->
        AlertDialog(
            onDismissRequest = { confirmDelete = null },
            title = { Text("Delete memory?") },
            text = { Text("Delete \"${entry.key}\"?") },
            confirmButton = {
                TextButton(onClick = {
                    scope.launch {
                        runCatching { container.gatewayClient.deleteMemory(config, entry.key) }
                            .onSuccess { reload() }
                            .onFailure { error = it.message }
                        confirmDelete = null
                    }
                }) { Text("Delete") }
            },
            dismissButton = {
                TextButton(onClick = { confirmDelete = null }) { Text("Cancel") }
            },
        )
    }

    if (showAdd) {
        var key by remember { mutableStateOf("") }
        var content by remember { mutableStateOf("") }
        var category by remember { mutableStateOf("") }
        var formError by remember { mutableStateOf<String?>(null) }

        AlertDialog(
            onDismissRequest = { showAdd = false },
            title = { Text("Add Memory") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(value = key, onValueChange = { key = it }, label = { Text("Key") })
                    OutlinedTextField(
                        value = content,
                        onValueChange = { content = it },
                        label = { Text("Content") },
                        minLines = 3,
                    )
                    OutlinedTextField(value = category, onValueChange = { category = it }, label = { Text("Category") })
                    formError?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    if (key.isBlank() || content.isBlank()) {
                        formError = "Key and content are required."
                        return@TextButton
                    }
                    scope.launch {
                        runCatching {
                            container.gatewayClient.storeMemory(
                                config,
                                key.trim(),
                                content.trim(),
                                category.trim().takeIf { it.isNotEmpty() },
                            )
                        }.onSuccess {
                            showAdd = false
                            reload()
                        }.onFailure { formError = it.message }
                    }
                }) { Text("Save") }
            },
            dismissButton = { TextButton(onClick = { showAdd = false }) { Text("Cancel") } },
        )
    }
}

private fun formatDate(iso: String): String = runCatching {
    SimpleDateFormat.getDateTimeInstance().format(Date(iso))
}.getOrElse { iso }
