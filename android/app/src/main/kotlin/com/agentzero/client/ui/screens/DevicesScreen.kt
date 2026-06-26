package com.agentzero.client.ui.screens

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
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
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
import androidx.compose.ui.unit.dp
import com.agentzero.client.AppContainer
import com.agentzero.client.data.model.PairedDevice
import com.agentzero.client.data.model.ServerConfig
import com.agentzero.client.ui.util.formatIsoDateTime
import kotlinx.coroutines.launch

@Composable
fun DevicesScreen(config: ServerConfig, container: AppContainer) {
    var devices by remember { mutableStateOf<List<PairedDevice>>(emptyList()) }
    var loading by remember { mutableStateOf(true) }
    var refreshing by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var inviteCode by remember { mutableStateOf<String?>(null) }
    var inviting by remember { mutableStateOf(false) }
    var pendingRevoke by remember { mutableStateOf<PairedDevice?>(null) }
    val scope = rememberCoroutineScope()

    fun load(refresh: Boolean = false) {
        scope.launch {
            if (refresh) refreshing = true else loading = true
            error = null
            runCatching { devices = container.gatewayClient.getPairedDevices(config) }
                .onFailure { error = it.message }
            loading = false
            refreshing = false
        }
    }

    LaunchedEffect(config) { load() }

    Column(Modifier.fillMaxSize().padding(12.dp)) {
        Row(
            Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("Paired Devices (${devices.size})", style = MaterialTheme.typography.titleMedium)
            Row {
                IconButton(
                    onClick = {
                        inviting = true
                        scope.launch {
                            runCatching {
                                inviteCode = container.gatewayClient.initiateDevicePairing(config)
                            }.onFailure { error = it.message }
                            inviting = false
                        }
                    },
                    enabled = !inviting,
                ) {
                    Icon(Icons.Default.Add, contentDescription = "Add device")
                }
                IconButton(onClick = { load(refresh = true) }, enabled = !refreshing) {
                    Icon(Icons.Default.Refresh, contentDescription = "Refresh")
                }
            }
        }

        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }

        inviteCode?.let { code ->
            Card(Modifier.fillMaxWidth().padding(vertical = 8.dp)) {
                Column(Modifier.padding(12.dp)) {
                    Text("New device pairing code", style = MaterialTheme.typography.labelMedium)
                    Text(code, style = MaterialTheme.typography.headlineMedium, fontFamily = FontFamily.Monospace)
                    Text(
                        "Enter this code on the new device. Valid for this gateway session only.",
                        style = MaterialTheme.typography.bodySmall,
                    )
                    TextButton(onClick = { inviteCode = null }) { Text("Dismiss") }
                }
            }
        }

        when {
            loading -> Column(
                Modifier.fillMaxSize(),
                verticalArrangement = Arrangement.Center,
                horizontalAlignment = Alignment.CenterHorizontally,
            ) { CircularProgressIndicator() }

            devices.isEmpty() -> Text("No paired devices found.", modifier = Modifier.padding(top = 24.dp))

            else -> LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                items(devices, key = { it.id }) { device ->
                    Card(Modifier.fillMaxWidth()) {
                        Column(Modifier.padding(12.dp)) {
                            Text(device.tokenFingerprint, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall)
                            Text("Paired by: ${device.pairedBy ?: "Unknown"}", style = MaterialTheme.typography.labelSmall)
                            Text("Created: ${formatIsoDateTime(device.createdAt)}", style = MaterialTheme.typography.labelSmall)
                            TextButton(onClick = { pendingRevoke = device }) {
                                Text("Revoke", color = MaterialTheme.colorScheme.error)
                            }
                        }
                    }
                }
            }
        }
    }

    pendingRevoke?.let { device ->
        AlertDialog(
            onDismissRequest = { pendingRevoke = null },
            title = { Text("Revoke device?") },
            text = { Text("Revoke ${device.tokenFingerprint}?") },
            confirmButton = {
                TextButton(onClick = {
                    scope.launch {
                        runCatching { container.gatewayClient.revokePairedDevice(config, device.id) }
                            .onSuccess {
                                devices = devices.filter { it.id != device.id }
                                pendingRevoke = null
                            }
                            .onFailure {
                                error = it.message
                                pendingRevoke = null
                            }
                    }
                }) { Text("Revoke") }
            },
            dismissButton = { TextButton(onClick = { pendingRevoke = null }) { Text("Cancel") } },
        )
    }
}

