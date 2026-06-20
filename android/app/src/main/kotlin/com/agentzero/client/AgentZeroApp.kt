package com.agentzero.client

import android.app.Application
import com.agentzero.client.data.AuthRepository
import com.agentzero.client.data.ChatWebSocketClient
import com.agentzero.client.data.GatewayClient
import com.agentzero.client.data.SettingsRepository
import com.agentzero.client.data.SseEventClient

class AgentZeroApp : Application() {
    lateinit var container: AppContainer
        private set

    override fun onCreate() {
        super.onCreate()
        container = AppContainer(this)
    }
}

class AppContainer(application: Application) {
    val settingsRepository = SettingsRepository(application)
    val authRepository = AuthRepository(application)

    var onUnauthorized: (() -> Unit)? = null

    val gatewayClient = GatewayClient(
        tokenProvider = { authRepository.getToken() },
        onUnauthorized = {
            authRepository.clearToken()
            onUnauthorized?.invoke()
        },
    )

    val chatWebSocketClient = ChatWebSocketClient(
        tokenProvider = { authRepository.getToken() },
        sessionIdProvider = { sessionIdCache ?: "default_session" },
    )

    val sseEventClient = SseEventClient(
        tokenProvider = { authRepository.getToken() },
    )

    var sessionIdCache: String? = null

    fun logout() {
        authRepository.clearToken()
    }
}
