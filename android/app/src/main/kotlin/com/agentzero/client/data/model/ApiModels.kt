package com.agentzero.client.data.model

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

@Serializable
data class PublicHealth(
    val status: String? = null,
    val paired: Boolean = false,
    @SerialName("require_pairing") val requirePairing: Boolean = true,
)

@Serializable
data class StatusResponse(
    val version: String,
    val provider: String? = null,
    val model: String,
    val temperature: Double = 0.0,
    @SerialName("uptime_seconds") val uptimeSeconds: Long = 0,
    @SerialName("gateway_port") val gatewayPort: Int = 0,
    val locale: String = "en",
    @SerialName("memory_backend") val memoryBackend: String = "",
    val paired: Boolean = false,
    val channels: Map<String, Boolean> = emptyMap(),
    val health: HealthSnapshot = HealthSnapshot(),
)

@Serializable
data class HealthSnapshot(
    val pid: Long = 0,
    @SerialName("updated_at") val updatedAt: String = "",
    @SerialName("uptime_seconds") val uptimeSeconds: Long = 0,
    val components: Map<String, ComponentHealth> = emptyMap(),
)

@Serializable
data class ComponentHealth(
    val status: String = "",
    @SerialName("updated_at") val updatedAt: String = "",
    @SerialName("last_ok") val lastOk: String? = null,
    @SerialName("last_error") val lastError: String? = null,
    @SerialName("restart_count") val restartCount: Int = 0,
)

@Serializable
data class CostSummary(
    @SerialName("hourly_cost_usd") val hourlyCostUsd: Double = 0.0,
    @SerialName("daily_cost_usd") val dailyCostUsd: Double = 0.0,
    @SerialName("monthly_cost_usd") val monthlyCostUsd: Double = 0.0,
    @SerialName("total_tokens") val totalTokens: Long = 0,
    @SerialName("request_count") val requestCount: Long = 0,
)

@Serializable
data class MemoryEntry(
    val id: String = "",
    val key: String,
    val content: String,
    val category: String = "",
    val timestamp: String = "",
    @SerialName("session_id") val sessionId: String? = null,
    val score: Double? = null,
)

@Serializable
data class MemoryStoreBody(
    val key: String,
    val content: String,
    val category: String? = null,
)

@Serializable
data class PairedDevice(
    val id: String,
    @SerialName("token_fingerprint") val tokenFingerprint: String,
    @SerialName("created_at") val createdAt: String? = null,
    @SerialName("paired_by") val pairedBy: String? = null,
)

@Serializable
data class WorkspaceTree(
    val workspace: String,
    val tree: List<WorkspaceFileNode> = emptyList(),
)

@Serializable
data class WorkspaceFileNode(
    val name: String,
    val path: String,
    val kind: String,
    val children: List<WorkspaceFileNode>? = null,
)

@Serializable
data class WorkspaceFileContent(
    val path: String,
    val content: String,
    val ext: String,
    val encoding: String? = null,
)

@Serializable
data class WsMessage(
    val type: String,
    val content: String? = null,
    @SerialName("full_response") val fullResponse: String? = null,
    val name: String? = null,
    val args: JsonElement? = null,
    val output: String? = null,
    val message: String? = null,
    @SerialName("session_id") val sessionId: String? = null,
    val messages: List<WsHistoryMessage>? = null,
)

@Serializable
data class WsHistoryMessage(
    val role: String,
    val content: String,
)

@Serializable
data class CronJob(
    val id: String,
    val name: String? = null,
    val command: String = "",
    val expression: String = "",
    val prompt: String? = null,
    @SerialName("job_type") val jobType: String = "",
    @SerialName("session_target") val sessionTarget: String = "",
    val model: String? = null,
    @SerialName("created_at") val createdAt: String = "",
    @SerialName("next_run") val nextRun: String = "",
    @SerialName("last_run") val lastRun: String? = null,
    @SerialName("last_status") val lastStatus: String? = null,
    @SerialName("last_output") val lastOutput: String? = null,
    val enabled: Boolean = true,
    @SerialName("light_context") val lightContext: Boolean = false,
)

@Serializable
data class CronAddBody(
    val name: String? = null,
    val command: String,
    val schedule: String,
    val enabled: Boolean = true,
)

@Serializable
data class SseEvent(
    val type: String = "message",
    val timestamp: String? = null,
    val message: String? = null,
    val content: String? = null,
    val data: String? = null,
    val direction: String? = null,
    val channel: String? = null,
    val signature: String? = null,
    val bearer: String? = null,
    val tool: String? = null,
    val output: String? = null,
    val success: Boolean? = null,
    @SerialName("duration_ms") val durationMs: Long? = null,
    val arguments: String? = null,
    val provider: String? = null,
    val model: String? = null,
    val iteration: Int? = null,
    val payload: JsonObject? = null,
)

data class ChatMessage(
    val id: String,
    val role: ChatRole,
    val content: String,
    val timestampMillis: Long = System.currentTimeMillis(),
)

enum class ChatRole { User, Agent }

data class LogEntry(
    val id: String,
    val event: SseEvent,
)

data class ServerConfig(
    val host: String,
    val port: Int,
) {
    val baseUrl: String
        get() {
            val trimmed = host.trim().removePrefix("http://").removePrefix("https://")
            return "http://$trimmed:$port"
        }
}
