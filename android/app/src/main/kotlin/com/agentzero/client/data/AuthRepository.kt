package com.agentzero.client.data

import android.content.Context
import android.content.SharedPreferences
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey
import com.agentzero.client.data.model.ServerConfig
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

class AuthRepository(context: Context) {
    private val prefs: SharedPreferences = EncryptedSharedPreferences.create(
        context,
        PREFS_NAME,
        MasterKey.Builder(context).setKeyScheme(MasterKey.KeyScheme.AES256_GCM).build(),
        EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
        EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
    )

    fun getToken(): String? = prefs.getString(TOKEN_KEY, null)?.takeIf { it.isNotBlank() }

    fun setToken(token: String) {
        prefs.edit().putString(TOKEN_KEY, token).apply()
    }

    fun clearToken() {
        prefs.edit().remove(TOKEN_KEY).apply()
    }

    suspend fun pair(config: ServerConfig, code: String, client: GatewayClient): Result<String> =
        withContext(Dispatchers.IO) {
            runCatching {
                val token = client.pair(config, code)
                setToken(token)
                token
            }
        }

    suspend fun resolveAuthenticated(
        config: ServerConfig,
        client: GatewayClient,
    ): AuthState = withContext(Dispatchers.IO) {
        val health = runCatching { client.getPublicHealth(config) }.getOrElse {
            return@withContext AuthState.ServerUnreachable(it.message ?: "Connection failed")
        }

        if (!health.requirePairing) {
            return@withContext AuthState.Authenticated
        }

        val token = getToken()
        if (token.isNullOrBlank()) {
            return@withContext AuthState.NeedsPairing
        }

        AuthState.Authenticated
    }

    sealed interface AuthState {
        data object Authenticated : AuthState
        data object NeedsPairing : AuthState
        data class ServerUnreachable(val message: String) : AuthState
    }

    companion object {
        private const val PREFS_NAME = "agentzero_auth"
        private const val TOKEN_KEY = "bearer_token"
    }
}
