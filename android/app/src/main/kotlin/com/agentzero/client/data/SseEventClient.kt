package com.agentzero.client.data

import com.agentzero.client.data.model.ServerConfig
import com.agentzero.client.data.model.SseEvent
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.launch
import kotlinx.serialization.json.Json
import okhttp3.Call
import okhttp3.OkHttpClient
import okhttp3.Request
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

class SseEventClient(
    private val tokenProvider: () -> String?,
) {
    private val json = Json { ignoreUnknownKeys = true }
    private val client = OkHttpClient.Builder()
        .connectTimeout(15, TimeUnit.SECONDS)
        .readTimeout(0, TimeUnit.MINUTES)
        .build()

    fun connect(config: ServerConfig): Flow<SseConnectionEvent> = callbackFlow {
        val activeCall = AtomicReference<Call?>(null)

        val job = launch(Dispatchers.IO) {
            val requestBuilder = Request.Builder()
                .url("${config.baseUrl}/api/events")
                .header("Accept", "text/event-stream")
                .get()

            tokenProvider()?.let { requestBuilder.header("Authorization", "Bearer $it") }

            val call = client.newCall(requestBuilder.build())
            activeCall.set(call)

            val response = runCatching { call.execute() }.getOrElse { error ->
                trySend(SseConnectionEvent.Error(error.message ?: "SSE connection failed"))
                close()
                return@launch
            }

            if (!response.isSuccessful) {
                trySend(SseConnectionEvent.Error("SSE connection failed: HTTP ${response.code}"))
                response.close()
                close()
                return@launch
            }

            val body = response.body
            if (body == null) {
                trySend(SseConnectionEvent.Error("SSE response has no body"))
                response.close()
                close()
                return@launch
            }

            trySend(SseConnectionEvent.Connected)

            val source = body.source()
            val buffer = StringBuilder()

            try {
                while (!source.exhausted()) {
                    val line = source.readUtf8Line() ?: break
                    when {
                        line.isEmpty() -> {
                            parseEvent(buffer.toString())?.let { trySend(SseConnectionEvent.Event(it)) }
                            buffer.clear()
                        }
                        line.startsWith(":") -> Unit
                        line.startsWith("event:") -> buffer.appendLine(line)
                        line.startsWith("data:") -> buffer.appendLine(line)
                        else -> buffer.appendLine(line)
                    }
                }
            } catch (e: Exception) {
                if (!call.isCanceled()) {
                    trySend(SseConnectionEvent.Error(e.message ?: "SSE stream error"))
                }
            } finally {
                body.close()
                trySend(SseConnectionEvent.Disconnected)
                close()
            }
        }

        awaitClose {
            activeCall.get()?.cancel()
            job.cancel()
        }
    }

    private fun parseEvent(raw: String): SseEvent? {
        if (raw.isBlank()) return null
        var eventType = "message"
        val dataLines = mutableListOf<String>()

        raw.lineSequence().forEach { line ->
            when {
                line.startsWith("event:") -> eventType = line.removePrefix("event:").trim()
                line.startsWith("data:") -> dataLines.add(line.removePrefix("data:").trim())
            }
        }

        if (dataLines.isEmpty()) return null
        val dataStr = dataLines.joinToString("\n")

        return runCatching {
            val parsed = json.decodeFromString(SseEvent.serializer(), dataStr)
            parsed.copy(type = parsed.type.ifBlank { eventType })
        }.getOrElse {
            SseEvent(type = eventType, data = dataStr)
        }
    }

    sealed interface SseConnectionEvent {
        data object Connected : SseConnectionEvent
        data object Disconnected : SseConnectionEvent
        data class Event(val event: SseEvent) : SseConnectionEvent
        data class Error(val message: String) : SseConnectionEvent
    }
}
