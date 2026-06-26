package com.agentzero.client.data

import com.agentzero.client.data.model.CostSummary
import com.agentzero.client.data.model.CronAddBody
import com.agentzero.client.data.model.CronJob
import com.agentzero.client.data.model.MemoryEntry
import com.agentzero.client.data.model.MemoryStoreBody
import com.agentzero.client.data.model.PairedDevice
import com.agentzero.client.data.model.PublicHealth
import com.agentzero.client.data.model.ServerConfig
import com.agentzero.client.data.model.StatusResponse
import com.agentzero.client.data.model.WorkspaceFileContent
import com.agentzero.client.data.model.WorkspaceTree
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.IOException
import java.util.concurrent.TimeUnit

class UnauthorizedException : IOException("Unauthorized")

class GatewayClient(
    private val tokenProvider: () -> String?,
    private val onUnauthorized: () -> Unit = {},
) {
    private val json = Json {
        ignoreUnknownKeys = true
        isLenient = true
    }

    private val client = OkHttpClient.Builder()
        .connectTimeout(15, TimeUnit.SECONDS)
        .readTimeout(60, TimeUnit.SECONDS)
        .writeTimeout(30, TimeUnit.SECONDS)
        .build()

    suspend fun getPublicHealth(config: ServerConfig): PublicHealth =
        getJson(config, "/health", auth = false, PublicHealth.serializer())

    suspend fun pair(config: ServerConfig, code: String): String {
        val request = Request.Builder()
            .url("${config.baseUrl}/pair")
            .post("".toRequestBody("application/json".toMediaType()))
            .header("X-Pairing-Code", code.trim())
            .build()

        val body = execute(request)
        val obj = json.parseToJsonElement(body).jsonObject
        return obj["token"]?.jsonPrimitive?.content
            ?: throw IOException("No token in /pair response")
    }

    suspend fun getStatus(config: ServerConfig): StatusResponse =
        getJson(config, "/api/status", auth = true, StatusResponse.serializer())

    suspend fun getCost(config: ServerConfig): CostSummary {
        val body = fetch(config, "/api/cost", auth = true)
        val element = json.parseToJsonElement(body)
        return if (element is JsonObject && "cost" in element) {
            json.decodeFromJsonElement(CostSummary.serializer(), element["cost"]!!)
        } else {
            json.decodeFromJsonElement(CostSummary.serializer(), element)
        }
    }

    suspend fun getMemory(
        config: ServerConfig,
        query: String? = null,
        category: String? = null,
    ): List<MemoryEntry> {
        val params = buildList {
            query?.takeIf { it.isNotBlank() }?.let { add("query=${encode(it)}") }
            category?.takeIf { it.isNotBlank() }?.let { add("category=${encode(it)}") }
        }
        val path = if (params.isEmpty()) "/api/memory" else "/api/memory?${params.joinToString("&")}"
        val body = fetch(config, path, auth = true)
        return unwrapList(body, "entries", MemoryEntry.serializer())
    }

    suspend fun storeMemory(
        config: ServerConfig,
        key: String,
        content: String,
        category: String?,
    ) {
        val payload = json.encodeToString(
            MemoryStoreBody.serializer(),
            MemoryStoreBody(key = key, content = content, category = category),
        )
        post(config, "/api/memory", payload, auth = true)
    }

    suspend fun deleteMemory(config: ServerConfig, key: String) {
        delete(config, "/api/memory/${encode(key)}", auth = true)
    }

    suspend fun getPairedDevices(config: ServerConfig): List<PairedDevice> {
        val body = fetch(config, "/api/pairing/devices", auth = true)
        return unwrapList(body, "devices", PairedDevice.serializer())
    }

    suspend fun revokePairedDevice(config: ServerConfig, id: String) {
        delete(config, "/api/pairing/devices/${encode(id)}", auth = true)
    }

    suspend fun initiateDevicePairing(config: ServerConfig): String {
        val body = post(config, "/api/pairing/initiate", "{}", auth = true)
        val obj = json.parseToJsonElement(body).jsonObject
        return obj["pairing_code"]?.jsonPrimitive?.content
            ?: throw IOException("No pairing_code in response")
    }

    suspend fun getWorkspaceFiles(config: ServerConfig): WorkspaceTree =
        getJson(config, "/api/workspace/files", auth = true, WorkspaceTree.serializer())

    suspend fun getWorkspaceFile(config: ServerConfig, path: String): WorkspaceFileContent {
        val body = fetch(config, "/api/workspace/file?path=${encode(path)}", auth = true)
        return json.decodeFromString(WorkspaceFileContent.serializer(), body)
    }

    suspend fun getCronJobs(config: ServerConfig): List<CronJob> {
        val body = fetch(config, "/api/cron", auth = true)
        return unwrapList(body, "jobs", CronJob.serializer())
    }

    suspend fun addCronJob(
        config: ServerConfig,
        name: String?,
        schedule: String,
        command: String,
    ): CronJob {
        val payload = json.encodeToString(
            CronAddBody.serializer(),
            CronAddBody(name = name, schedule = schedule, command = command),
        )
        val body = post(config, "/api/cron", payload, auth = true)
        val element = json.parseToJsonElement(body)
        if (element is JsonObject && element["job"] != null) {
            return json.decodeFromJsonElement(CronJob.serializer(), element["job"]!!)
        }
        return json.decodeFromString(CronJob.serializer(), body)
    }

    suspend fun deleteCronJob(config: ServerConfig, id: String) {
        delete(config, "/api/cron/${encode(id)}", auth = true)
    }

    private suspend inline fun <reified T> getJson(
        config: ServerConfig,
        path: String,
        auth: Boolean,
        deserializer: kotlinx.serialization.KSerializer<T>,
    ): T {
        val body = fetch(config, path, auth)
        return json.decodeFromString(deserializer, body)
    }

    private suspend fun fetch(config: ServerConfig, path: String, auth: Boolean): String {
        val request = Request.Builder()
            .url("${config.baseUrl}$path")
            .get()
            .apply { if (auth) addAuth(this) }
            .build()
        return execute(request)
    }

    private suspend fun post(config: ServerConfig, path: String, body: String, auth: Boolean): String {
        val request = Request.Builder()
            .url("${config.baseUrl}$path")
            .post(body.toRequestBody("application/json".toMediaType()))
            .apply { if (auth) addAuth(this) }
            .build()
        return execute(request)
    }

    private suspend fun delete(config: ServerConfig, path: String, auth: Boolean) {
        val request = Request.Builder()
            .url("${config.baseUrl}$path")
            .delete()
            .apply { if (auth) addAuth(this) }
            .build()
        execute(request)
    }

    private fun addAuth(builder: Request.Builder) {
        tokenProvider()?.let { builder.header("Authorization", "Bearer $it") }
    }

    private suspend fun <T> unwrapList(
        body: String,
        key: String,
        serializer: kotlinx.serialization.KSerializer<T>,
    ): List<T> {
        val element = json.parseToJsonElement(body)
        val array: JsonArray = when {
            element is JsonArray -> element
            element is JsonObject && element[key] is JsonArray -> element[key]!!.jsonArray
            else -> return emptyList()
        }
        return array.map { json.decodeFromJsonElement(serializer, it) }
    }

    private suspend fun execute(request: Request): String = withContext(Dispatchers.IO) {
        client.newCall(request).execute().use { response ->
            if (response.code == 401) {
                onUnauthorized()
                throw UnauthorizedException()
            }
            if (!response.isSuccessful) {
                val text = response.body?.string().orEmpty()
                throw IOException("HTTP ${response.code}: ${text.ifBlank { response.message }}")
            }
            if (response.code == 204) return@withContext ""
            return@withContext response.body?.string().orEmpty()
        }
    }

    private fun encode(value: String): String = java.net.URLEncoder.encode(value, Charsets.UTF_8.name())
}
