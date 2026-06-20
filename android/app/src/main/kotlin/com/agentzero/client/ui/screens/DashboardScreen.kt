package com.agentzero.client.ui.screens

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ExpandLess
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.agentzero.client.AppContainer
import com.agentzero.client.data.model.CostSummary
import com.agentzero.client.data.model.ServerConfig
import com.agentzero.client.data.model.StatusResponse
import java.util.Locale

@Composable
fun DashboardScreen(config: ServerConfig, container: AppContainer) {
    var status by remember { mutableStateOf<StatusResponse?>(null) }
    var cost by remember { mutableStateOf<CostSummary?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var loading by remember { mutableStateOf(true) }

    LaunchedEffect(config) {
        loading = true
        error = null
        runCatching {
            val s = container.gatewayClient.getStatus(config)
            val c = container.gatewayClient.getCost(config)
            status = s
            cost = c
        }.onFailure { error = it.message }
        loading = false
    }

    when {
        loading -> Column(
            Modifier.fillMaxSize(),
            verticalArrangement = Arrangement.Center,
            horizontalAlignment = Alignment.CenterHorizontally,
        ) { CircularProgressIndicator() }

        error != null -> Text(
            text = error ?: "",
            color = MaterialTheme.colorScheme.error,
            modifier = Modifier.padding(16.dp),
        )

        status != null && cost != null -> DashboardContent(status!!, cost!!)
    }
}

@Composable
private fun DashboardContent(status: StatusResponse, cost: CostSummary) {
    val maxCost = maxOf(cost.hourlyCostUsd, cost.dailyCostUsd, cost.monthlyCostUsd, 0.001)
    var costOpen by remember { mutableStateOf(true) }
    var tokensOpen by remember { mutableStateOf(true) }
    var healthOpen by remember { mutableStateOf(true) }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("Runtime Dashboard", style = MaterialTheme.typography.headlineSmall)
        Text("v${status.version}", style = MaterialTheme.typography.labelMedium)
        Text(
            if (status.paired) "Paired" else "Unpaired",
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.primary,
        )

        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            MetricCard("Provider", status.provider ?: "Unknown", Modifier.weight(1f))
            MetricCard("Model", status.model, Modifier.weight(1f))
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            MetricCard("Uptime", formatUptime(status.uptimeSeconds), Modifier.weight(1f))
            MetricCard("Port", ":${status.gatewayPort}", Modifier.weight(1f))
        }
        MetricCard("Memory Backend", status.memoryBackend.replaceFirstChar {
            if (it.isLowerCase()) it.titlecase(Locale.getDefault()) else it.toString()
        }, Modifier.fillMaxWidth())

        CollapsibleCard("Cost Pulse", costOpen, { costOpen = !costOpen }) {
            listOf(
                "Hourly" to cost.hourlyCostUsd,
                "Daily" to cost.dailyCostUsd,
                "Monthly" to cost.monthlyCostUsd,
            ).forEach { (label, value) ->
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                    Text(label)
                    Text(formatUsd(value), fontWeight = FontWeight.SemiBold)
                }
                LinearProgressIndicator(
                    progress = { (value / maxCost).toFloat().coerceIn(0.03f, 1f) },
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(vertical = 4.dp),
                )
            }
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                Text("Total Tokens: ${cost.totalTokens}")
                Text("Requests: ${cost.requestCount}")
            }
        }

        CollapsibleCard("Token Statistics", tokensOpen, { tokensOpen = !tokensOpen }) {
            val avg = if (cost.requestCount > 0) cost.totalTokens / cost.requestCount else 0
            val costPer1k = if (cost.totalTokens > 0) {
                (cost.monthlyCostUsd / cost.totalTokens) * 1000
            } else 0.0
            Text("Total Tokens: ${cost.totalTokens}")
            Text("Avg Tokens / Request: $avg")
            Text("Cost per 1K Tokens: ${formatUsd(costPer1k)}")
        }

        CollapsibleCard("Component Health", healthOpen, { healthOpen = !healthOpen }) {
            if (status.health.components.isEmpty()) {
                Text("No component health is currently available.")
            } else {
                status.health.components.forEach { (name, component) ->
                    Card(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(vertical = 4.dp),
                        colors = CardDefaults.cardColors(
                            containerColor = MaterialTheme.colorScheme.surfaceVariant,
                        ),
                    ) {
                        Column(Modifier.padding(12.dp)) {
                            Text(name.replaceFirstChar {
                                if (it.isLowerCase()) it.titlecase(Locale.getDefault()) else it.toString()
                            }, fontWeight = FontWeight.SemiBold)
                            Text(component.status)
                            if (component.restartCount > 0) {
                                Text("Restarts: ${component.restartCount}")
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun MetricCard(title: String, value: String, modifier: Modifier = Modifier) {
    Card(modifier = modifier, colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant)) {
        Column(Modifier.padding(12.dp)) {
            Text(title, style = MaterialTheme.typography.labelSmall)
            Spacer(Modifier.height(4.dp))
            Text(value, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
        }
    }
}

@Composable
private fun CollapsibleCard(
    title: String,
    expanded: Boolean,
    onToggle: () -> Unit,
    content: @Composable () -> Unit,
) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(12.dp)) {
            Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(title, style = MaterialTheme.typography.titleMedium)
                IconButton(onClick = onToggle) {
                    Icon(
                        if (expanded) Icons.Default.ExpandLess else Icons.Default.ExpandMore,
                        contentDescription = null,
                    )
                }
            }
            AnimatedVisibility(expanded) {
                Column(Modifier.padding(top = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    content()
                }
            }
        }
    }
}

private fun formatUptime(seconds: Long): String {
    val d = seconds / 86400
    val h = (seconds % 86400) / 3600
    val m = (seconds % 3600) / 60
    return when {
        d > 0 -> "${d}d ${h}h ${m}m"
        h > 0 -> "${h}h ${m}m"
        else -> "${m}m"
    }
}

private fun formatUsd(value: Double): String = String.format(Locale.US, "$%.4f", value)
