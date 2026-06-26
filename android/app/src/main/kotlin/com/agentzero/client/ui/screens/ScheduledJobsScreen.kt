package com.agentzero.client.ui.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
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
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.agentzero.client.AppContainer
import com.agentzero.client.data.model.CronJob
import com.agentzero.client.data.model.ServerConfig
import com.agentzero.client.ui.util.formatIsoDateTime
import kotlinx.coroutines.launch

@Composable
fun ScheduledJobsScreen(config: ServerConfig, container: AppContainer) {
    var jobs by remember { mutableStateOf<List<CronJob>>(emptyList()) }
    var loading by remember { mutableStateOf(true) }
    var refreshing by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var selected by remember { mutableStateOf<CronJob?>(null) }
    var showAdd by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf<CronJob?>(null) }
    val scope = rememberCoroutineScope()

    fun reload(refresh: Boolean = false) {
        scope.launch {
            if (refresh) refreshing = true else loading = true
            error = null
            runCatching { jobs = container.gatewayClient.getCronJobs(config) }
                .onFailure { error = it.message }
            loading = false
            refreshing = false
        }
    }

    LaunchedEffect(config) { reload() }

    Scaffold(
        floatingActionButton = {
            FloatingActionButton(onClick = { showAdd = true }) {
                Icon(Icons.Default.Add, contentDescription = "Add scheduled job")
            }
        },
    ) { padding ->
        Column(
            Modifier
                .fillMaxSize()
                .padding(padding)
                .padding(12.dp),
        ) {
            Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    "Scheduled Jobs (${jobs.size})",
                    style = MaterialTheme.typography.titleMedium,
                )
                IconButton(onClick = { reload(refresh = true) }, enabled = !refreshing) {
                    Icon(Icons.Default.Refresh, contentDescription = "Refresh")
                }
            }

            error?.let {
                Text(it, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(top = 8.dp))
            }

            when {
                loading -> Column(
                    Modifier.fillMaxSize(),
                    verticalArrangement = Arrangement.Center,
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) { CircularProgressIndicator() }

                jobs.isEmpty() -> Text(
                    "No scheduled jobs configured.",
                    modifier = Modifier.padding(top = 24.dp),
                )

                else -> LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    items(jobs, key = { it.id }) { job ->
                        Card(
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable { selected = job },
                        ) {
                            Row(
                                Modifier
                                    .fillMaxWidth()
                                    .padding(12.dp),
                                horizontalArrangement = Arrangement.SpaceBetween,
                                verticalAlignment = Alignment.Top,
                            ) {
                                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                                    Text(
                                        job.name?.takeIf { it.isNotBlank() } ?: "Untitled job",
                                        style = MaterialTheme.typography.titleSmall,
                                    )
                                    Text(
                                        job.expression,
                                        fontFamily = FontFamily.Monospace,
                                        style = MaterialTheme.typography.labelSmall,
                                    )
                                    Text(
                                        "Next: ${formatIsoDateTime(job.nextRun)}",
                                        style = MaterialTheme.typography.labelSmall,
                                    )
                                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                        StatusChip(
                                            label = if (job.enabled) "Enabled" else "Disabled",
                                            enabled = job.enabled,
                                        )
                                        job.lastStatus?.takeIf { it.isNotBlank() }?.let { status ->
                                            Text(
                                                status.replaceFirstChar { it.uppercase() },
                                                style = MaterialTheme.typography.labelSmall,
                                            )
                                        }
                                    }
                                }
                                IconButton(onClick = { confirmDelete = job }) {
                                    Icon(Icons.Default.Delete, contentDescription = "Delete job")
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    selected?.let { job ->
        AlertDialog(
            onDismissRequest = { selected = null },
            confirmButton = { TextButton(onClick = { selected = null }) { Text("Close") } },
            title = { Text(job.name?.takeIf { it.isNotBlank() } ?: "Scheduled Job") },
            text = {
                Column(
                    Modifier
                        .fillMaxWidth()
                        .heightIn(max = 420.dp)
                        .verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    DetailLine("ID", job.id)
                    DetailLine("Schedule", job.expression, monospace = true)
                    DetailLine("Type", job.jobType.replaceFirstChar { it.uppercase() })
                    DetailLine("Session", job.sessionTarget.replaceFirstChar { it.uppercase() })
                    job.model?.takeIf { it.isNotBlank() }?.let { DetailLine("Model", it, monospace = true) }
                    DetailLine("Command", job.command.ifBlank { "—" }, monospace = true)
                    job.prompt?.takeIf { it.isNotBlank() }?.let { DetailLine("Prompt", it, monospace = true) }
                    DetailLine("Created", formatIsoDateTime(job.createdAt))
                    DetailLine("Next run", formatIsoDateTime(job.nextRun))
                    DetailLine("Last run", formatIsoDateTime(job.lastRun))
                    job.lastStatus?.let { DetailLine("Last status", it.replaceFirstChar { c -> c.uppercase() }) }
                    job.lastOutput?.takeIf { it.isNotBlank() }?.let {
                        DetailLine("Last output", it, monospace = true)
                    }
                    DetailLine("Light context", if (job.lightContext) "On" else "Off")
                }
            },
        )
    }

    confirmDelete?.let { job ->
        AlertDialog(
            onDismissRequest = { confirmDelete = null },
            title = { Text("Delete scheduled job?") },
            text = { Text("Delete \"${job.name ?: job.id}\"?") },
            confirmButton = {
                TextButton(onClick = {
                    scope.launch {
                        runCatching { container.gatewayClient.deleteCronJob(config, job.id) }
                            .onSuccess {
                                jobs = jobs.filter { it.id != job.id }
                                confirmDelete = null
                            }
                            .onFailure {
                                error = it.message
                                confirmDelete = null
                            }
                    }
                }) { Text("Delete") }
            },
            dismissButton = { TextButton(onClick = { confirmDelete = null }) { Text("Cancel") } },
        )
    }

    if (showAdd) {
        var name by remember { mutableStateOf("") }
        var schedule by remember { mutableStateOf("") }
        var command by remember { mutableStateOf("") }
        var formError by remember { mutableStateOf<String?>(null) }
        var submitting by remember { mutableStateOf(false) }

        AlertDialog(
            onDismissRequest = { if (!submitting) showAdd = false },
            title = { Text("Add Scheduled Job") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(
                        value = name,
                        onValueChange = { name = it },
                        label = { Text("Name (optional)") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                    )
                    OutlinedTextField(
                        value = schedule,
                        onValueChange = { schedule = it },
                        label = { Text("Schedule (cron)") },
                        placeholder = { Text("0 0 * * *") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                    )
                    OutlinedTextField(
                        value = command,
                        onValueChange = { command = it },
                        label = { Text("Command") },
                        minLines = 2,
                        modifier = Modifier.fillMaxWidth(),
                    )
                    formError?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                }
            },
            confirmButton = {
                TextButton(
                    enabled = !submitting,
                    onClick = {
                        if (schedule.isBlank() || command.isBlank()) {
                            formError = "Schedule and command are required."
                            return@TextButton
                        }
                        submitting = true
                        scope.launch {
                            runCatching {
                                container.gatewayClient.addCronJob(
                                    config,
                                    name = name.trim().takeIf { it.isNotEmpty() },
                                    schedule = schedule.trim(),
                                    command = command.trim(),
                                )
                            }.onSuccess {
                                showAdd = false
                                reload(refresh = true)
                            }.onFailure {
                                formError = it.message
                            }
                            submitting = false
                        }
                    },
                ) { Text(if (submitting) "Adding…" else "Add") }
            },
            dismissButton = {
                TextButton(onClick = { showAdd = false }, enabled = !submitting) { Text("Cancel") }
            },
        )
    }
}

@Composable
private fun StatusChip(label: String, enabled: Boolean) {
    Surface(
        color = if (enabled) {
            MaterialTheme.colorScheme.primaryContainer
        } else {
            MaterialTheme.colorScheme.surfaceVariant
        },
        shape = MaterialTheme.shapes.small,
    ) {
        Text(
            label,
            modifier = Modifier.padding(horizontal = 8.dp, vertical = 2.dp),
            style = MaterialTheme.typography.labelSmall,
        )
    }
}

@Composable
private fun DetailLine(label: String, value: String, monospace: Boolean = false) {
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(label, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(
            value,
            style = MaterialTheme.typography.bodySmall,
            fontFamily = if (monospace) FontFamily.Monospace else FontFamily.Default,
            maxLines = if (monospace) Int.MAX_VALUE else 2,
            overflow = if (monospace) TextOverflow.Visible else TextOverflow.Ellipsis,
        )
    }
}
