package com.agentzero.client.data

import android.content.Context
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.intPreferencesKey
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import com.agentzero.client.data.model.ServerConfig
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map

private val Context.dataStore: DataStore<Preferences> by preferencesDataStore(name = "server_settings")

class SettingsRepository(private val context: Context) {
    private val hostKey = stringPreferencesKey("gateway_host")
    private val portKey = intPreferencesKey("gateway_port")
    private val wsSessionKey = stringPreferencesKey("ws_session_id")

    val serverConfig: Flow<ServerConfig?> = context.dataStore.data.map { prefs ->
        val host = prefs[hostKey]
        val port = prefs[portKey]
        if (host.isNullOrBlank() || port == null || port <= 0) {
            null
        } else {
            ServerConfig(host = host, port = port)
        }
    }

    val wsSessionId: Flow<String?> = context.dataStore.data.map { prefs ->
        prefs[wsSessionKey]
    }

    suspend fun saveServerConfig(host: String, port: Int) {
        context.dataStore.edit { prefs ->
            prefs[hostKey] = host.trim()
            prefs[portKey] = port
        }
    }

    suspend fun saveWsSessionId(sessionId: String) {
        context.dataStore.edit { prefs ->
            prefs[wsSessionKey] = sessionId
        }
    }

    suspend fun clearServerConfig() {
        context.dataStore.edit { prefs ->
            prefs.remove(hostKey)
            prefs.remove(portKey)
        }
    }
}
