package com.agentzero.client

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import com.agentzero.client.ui.AppViewModel
import com.agentzero.client.ui.ErrorScreen
import com.agentzero.client.ui.LoadingScreen
import com.agentzero.client.ui.MainShell
import com.agentzero.client.ui.RootUiState
import com.agentzero.client.ui.screens.PairingScreen
import com.agentzero.client.ui.screens.ServerSetupScreen
import com.agentzero.client.ui.theme.AgentZeroTheme

class MainActivity : ComponentActivity() {
    private val container by lazy { (application as AgentZeroApp).container }
    private val viewModel: AppViewModel by viewModels { AppViewModel.Factory(container) }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()

        setContent {
            AgentZeroTheme {
                val rootState by viewModel.rootState.collectAsState()

                when (val state = rootState) {
                    RootUiState.Loading -> LoadingScreen()

                    RootUiState.NeedsServerSetup -> ServerSetupScreen(
                        onSave = viewModel::saveServer,
                    )

                    is RootUiState.NeedsPairing -> PairingScreen(
                        config = state.config,
                        onPair = viewModel::pair,
                    )

                    is RootUiState.Ready -> MainShell(
                        config = state.config,
                        container = container,
                        onLogout = viewModel::logout,
                        onChangeServer = viewModel::clearServerAndLogout,
                    )

                    is RootUiState.Error -> ErrorScreen(
                        message = state.message,
                        onRetry = viewModel::retryConnection,
                    )
                }
            }
        }
    }
}
