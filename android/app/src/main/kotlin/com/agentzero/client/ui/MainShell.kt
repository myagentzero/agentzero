package com.agentzero.client.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Logout
import androidx.compose.material.icons.filled.Chat
import androidx.compose.material.icons.filled.Dashboard
import androidx.compose.material.icons.filled.Devices
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.filled.Memory
import androidx.compose.material.icons.filled.MonitorHeart
import androidx.compose.material.icons.filled.Menu
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.NavigationDrawerItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.unit.dp
import com.agentzero.client.AppContainer
import com.agentzero.client.data.model.ServerConfig
import com.agentzero.client.ui.screens.AgentChatScreen
import com.agentzero.client.ui.screens.DashboardScreen
import com.agentzero.client.ui.screens.DevicesScreen
import com.agentzero.client.ui.screens.MemoryScreen
import com.agentzero.client.ui.screens.MissionControlScreen
import com.agentzero.client.ui.screens.WorkspaceScreen
import kotlinx.coroutines.launch

enum class MainDestination(
    val title: String,
    val icon: ImageVector,
) {
    Dashboard("Dashboard", Icons.Default.Dashboard),
    AgentChat("Agent Chat", Icons.Default.Chat),
    MissionControl("Mission Control", Icons.Default.MonitorHeart),
    Memory("Memory", Icons.Default.Memory),
    Workspace("Workspace", Icons.Default.Folder),
    Devices("Devices", Icons.Default.Devices),
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MainShell(
    config: ServerConfig,
    container: AppContainer,
    onLogout: () -> Unit,
    onChangeServer: () -> Unit,
) {
    var destination by remember { mutableStateOf(MainDestination.Dashboard) }
    var showSettings by remember { mutableStateOf(false) }
    val drawerState = rememberDrawerState(DrawerValue.Closed)
    val scope = rememberCoroutineScope()

    ModalNavigationDrawer(
        drawerState = drawerState,
        drawerContent = {
            ModalDrawerSheet {
                Text(
                    "AgentZero",
                    modifier = Modifier.padding(horizontal = 28.dp, vertical = 24.dp),
                )
                MainDestination.entries.forEach { item ->
                    NavigationDrawerItem(
                        label = { Text(item.title) },
                        selected = destination == item,
                        icon = { Icon(item.icon, contentDescription = item.title) },
                        onClick = {
                            destination = item
                            scope.launch { drawerState.close() }
                        },
                    )
                }
            }
        },
    ) {
        Scaffold(
            topBar = {
                TopAppBar(
                    title = { Text(destination.title) },
                    navigationIcon = {
                        IconButton(onClick = { scope.launch { drawerState.open() } }) {
                            Icon(Icons.Default.Menu, contentDescription = "Menu")
                        }
                    },
                    actions = {
                        IconButton(onClick = { showSettings = true }) {
                            Icon(Icons.Default.Settings, contentDescription = "Settings")
                        }
                    },
                )
            },
        ) { padding ->
            Box(Modifier.fillMaxSize().padding(padding)) {
                when (destination) {
                    MainDestination.Dashboard -> DashboardScreen(config, container)
                    MainDestination.AgentChat -> AgentChatScreen(config, container)
                    MainDestination.MissionControl -> MissionControlScreen(config, container)
                    MainDestination.Memory -> MemoryScreen(config, container)
                    MainDestination.Workspace -> WorkspaceScreen(config, container)
                    MainDestination.Devices -> DevicesScreen(config, container)
                }
            }
        }
    }

    if (showSettings) {
        AlertDialog(
            onDismissRequest = { showSettings = false },
            title = { Text("Settings") },
            text = {
                Text("Server: ${config.baseUrl}")
            },
            confirmButton = {
                TextButton(onClick = {
                    showSettings = false
                    onLogout()
                }) {
                    Icon(Icons.AutoMirrored.Filled.Logout, contentDescription = null)
                    Text("Sign out")
                }
            },
            dismissButton = {
                TextButton(onClick = {
                    showSettings = false
                    onChangeServer()
                }) { Text("Change server") }
            },
        )
    }
}

@Composable
fun LoadingScreen() {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        CircularProgressIndicator()
    }
}

@Composable
fun ErrorScreen(message: String, onRetry: () -> Unit) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        ColumnHorizontal(message, onRetry)
    }
}

@Composable
private fun ColumnHorizontal(message: String, onRetry: () -> Unit) {
    androidx.compose.foundation.layout.Column(horizontalAlignment = Alignment.CenterHorizontally) {
        Text(message)
        TextButton(onClick = onRetry) { Text("Retry") }
    }
}
