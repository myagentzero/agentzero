package com.agentzero.client.ui

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import com.agentzero.client.AppContainer
import com.agentzero.client.data.AuthRepository
import com.agentzero.client.data.model.ServerConfig
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch

sealed interface RootUiState {
    data object Loading : RootUiState
    data object NeedsServerSetup : RootUiState
    data class NeedsPairing(val config: ServerConfig) : RootUiState
    data class Ready(val config: ServerConfig) : RootUiState
    data class Error(val message: String) : RootUiState
}

class AppViewModel(
    private val container: AppContainer,
) : ViewModel() {
    private val _rootState = MutableStateFlow<RootUiState>(RootUiState.Loading)
    val rootState: StateFlow<RootUiState> = _rootState.asStateFlow()

    val serverConfig = container.settingsRepository.serverConfig.stateIn(
        viewModelScope,
        SharingStarted.WhileSubscribed(5_000),
        null,
    )

    init {
        container.onUnauthorized = {
            viewModelScope.launch {
                serverConfig.value?.let { config ->
                    _rootState.value = RootUiState.NeedsPairing(config)
                }
            }
        }
        viewModelScope.launch {
            container.settingsRepository.serverConfig.collect { config ->
                refreshAuth(config)
            }
        }
    }

    fun saveServer(host: String, port: Int, onDone: (String?) -> Unit) {
        if (host.isBlank() || port !in 1..65535) {
            onDone("Enter a valid host and port (1–65535).")
            return
        }
        viewModelScope.launch {
            container.settingsRepository.saveServerConfig(host, port)
            onDone(null)
        }
    }

    fun pair(code: String, config: ServerConfig, onDone: (String?) -> Unit) {
        if (code.trim().length < 6) {
            onDone("Enter the 6-digit pairing code.")
            return
        }
        viewModelScope.launch {
            container.authRepository.pair(config, code.trim(), container.gatewayClient)
                .onSuccess {
                    _rootState.value = RootUiState.Ready(config)
                    onDone(null)
                }
                .onFailure { onDone(it.message ?: "Pairing failed") }
        }
    }

    fun logout() {
        container.logout()
        viewModelScope.launch {
            refreshAuth(serverConfig.value)
        }
    }

    fun clearServerAndLogout() {
        viewModelScope.launch {
            container.logout()
            container.settingsRepository.clearServerConfig()
        }
    }

    fun retryConnection() {
        viewModelScope.launch {
            refreshAuth(serverConfig.value)
        }
    }

    private suspend fun refreshAuth(config: ServerConfig?) {
        if (config == null) {
            _rootState.value = RootUiState.NeedsServerSetup
            return
        }

        _rootState.value = RootUiState.Loading
        when (val state = container.authRepository.resolveAuthenticated(config, container.gatewayClient)) {
            is AuthRepository.AuthState.Authenticated ->
                _rootState.value = RootUiState.Ready(config)

            is AuthRepository.AuthState.NeedsPairing ->
                _rootState.value = RootUiState.NeedsPairing(config)

            is AuthRepository.AuthState.ServerUnreachable ->
                _rootState.value = RootUiState.Error(state.message)
        }
    }

    class Factory(private val container: AppContainer) : ViewModelProvider.Factory {
        @Suppress("UNCHECKED_CAST")
        override fun <T : ViewModel> create(modelClass: Class<T>): T {
            if (modelClass.isAssignableFrom(AppViewModel::class.java)) {
                return AppViewModel(container) as T
            }
            throw IllegalArgumentException("Unknown ViewModel: ${modelClass.name}")
        }
    }
}
