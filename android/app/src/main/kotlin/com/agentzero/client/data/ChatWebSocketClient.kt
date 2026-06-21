package com.agentzero.client.data

import com.agentzero.client.data.model.ServerConfig
import com.agentzero.client.data.model.WsMessage
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.serialization.builtins.serializer
import kotlinx.serialization.json.Json
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import java.util.UUID
import java.util.concurrent.TimeUnit

class ChatWebSocketClient(
    private val tokenProvider: () -> String?,
    private val sessionIdProvider: () -> String,
) {
    private val json = Json { ignoreUnknownKeys = true }
    private val client = OkHttpClient.Builder()
        .connectTimeout(15, TimeUnit.SECONDS)
        .readTimeout(0, TimeUnit.MINUTES)
        .pingInterval(30, TimeUnit.SECONDS)
        .build()

    private var webSocket: WebSocket? = null

    fun connect(config: ServerConfig): Flow<ChatWsEvent> = callbackFlow {
        val sessionId = sessionIdProvider()
        val token = tokenProvider()
        val url = "${config.baseUrl.replace("http://", "ws://").replace("https://", "wss://")}" +
            "/ws/chat?session_id=${java.net.URLEncoder.encode(sessionId, Charsets.UTF_8.name())}"

        val protocols = buildList {
            add("agentzero.v1")
            if (!token.isNullOrBlank()) add("bearer.$token")
        }

        val request = Request.Builder()
            .url(url)
            .header("Sec-WebSocket-Protocol", protocols.joinToString(", "))
            .build()
        val listener = object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                trySend(ChatWsEvent.Connected)
            }

            override fun onMessage(webSocket: WebSocket, text: String) {
                runCatching {
                    json.decodeFromString(WsMessage.serializer(), text)
                }.onSuccess { trySend(ChatWsEvent.Message(it)) }
            }

            override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                webSocket.close(code, reason)
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                trySend(ChatWsEvent.Disconnected)
                close()
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                trySend(ChatWsEvent.Error(t.message ?: "WebSocket error"))
                close(t)
            }
        }

        webSocket = client.newWebSocket(request, listener)

        awaitClose {
            webSocket?.close(1000, "Client disconnect")
            webSocket = null
        }
    }

    fun sendMessage(content: String): Boolean {
        val payload = """{"type":"message","content":${json.encodeToString(String.serializer(), content)}}"""
        return webSocket?.send(payload) == true
    }

    fun newSessionId(): String =
        UUID.randomUUID().toString().replace("-", "_")

    sealed interface ChatWsEvent {
        data object Connected : ChatWsEvent
        data object Disconnected : ChatWsEvent
        data class Message(val message: WsMessage) : ChatWsEvent
        data class Error(val message: String) : ChatWsEvent
    }
}
