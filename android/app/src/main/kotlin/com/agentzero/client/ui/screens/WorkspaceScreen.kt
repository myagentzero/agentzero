package com.agentzero.client.ui.screens

import android.content.ContentValues
import android.content.Context
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import android.widget.Toast
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ChevronRight
import androidx.compose.material.icons.filled.Description
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.filled.FolderOpen
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import com.agentzero.client.AppContainer
import com.agentzero.client.data.model.ServerConfig
import com.agentzero.client.data.model.WorkspaceFileContent
import com.agentzero.client.data.model.WorkspaceFileNode
import com.agentzero.client.data.model.WorkspaceTree
import kotlinx.coroutines.launch
import org.json.JSONObject
import java.util.Base64

private val VIEWABLE = setOf("md", "json", "jsonl", "js", "py", "ps1", "license", "txt", "svg")
private val DOWNLOADABLE = setOf("pdf", "docx", "xlsx", "pptx", "png", "jpg", "jpeg", "gif", "zip", "tar", "gz")

@Composable
fun WorkspaceScreen(config: ServerConfig, container: AppContainer) {
    var treeData by remember { mutableStateOf<WorkspaceTree?>(null) }
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    var activeFile by remember { mutableStateOf<WorkspaceFileContent?>(null) }
    var fileLoading by remember { mutableStateOf(false) }
    var fileError by remember { mutableStateOf<String?>(null) }
    val context = LocalContext.current
    val scope = rememberCoroutineScope()

    LaunchedEffect(config) {
        loading = true
        error = null
        runCatching { treeData = container.gatewayClient.getWorkspaceFiles(config) }
            .onFailure { error = it.message }
        loading = false
    }

    fun openFile(node: WorkspaceFileNode) {
        scope.launch {
            fileLoading = true
            fileError = null
            activeFile = null
            runCatching { activeFile = container.gatewayClient.getWorkspaceFile(config, node.path) }
                .onFailure { fileError = it.message }
            fileLoading = false
        }
    }

    fun downloadFile(node: WorkspaceFileNode) {
        scope.launch {
            fileLoading = true
            fileError = null
            runCatching {
                val file = container.gatewayClient.getWorkspaceFile(config, node.path)
                saveDownload(context, node.name, file)
                Toast.makeText(context, "Saved ${node.name} to Downloads", Toast.LENGTH_SHORT).show()
            }.onFailure { fileError = it.message }
            fileLoading = false
        }
    }

    Column(Modifier.fillMaxSize().padding(12.dp)) {
        treeData?.let { Text(it.workspace, style = MaterialTheme.typography.labelSmall, fontFamily = FontFamily.Monospace) }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }

        when {
            loading -> Column(
                Modifier.fillMaxSize(),
                verticalArrangement = Arrangement.Center,
                horizontalAlignment = Alignment.CenterHorizontally,
            ) { CircularProgressIndicator() }

            treeData != null -> Row(Modifier.fillMaxSize(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Card(Modifier.weight(0.4f)) {
                    LazyColumn(Modifier.padding(8.dp)) {
                        treeData!!.tree.forEach { node ->
                            item {
                                WorkspaceTreeNode(
                                    node = node,
                                    depth = 0,
                                    onOpen = ::openFile,
                                    onDownload = ::downloadFile,
                                )
                            }
                        }
                    }
                }
                Card(Modifier.weight(0.6f)) {
                    when {
                        fileLoading -> Column(
                            Modifier.fillMaxSize(),
                            verticalArrangement = Arrangement.Center,
                            horizontalAlignment = Alignment.CenterHorizontally,
                        ) { CircularProgressIndicator() }

                        fileError != null -> Text(fileError!!, Modifier.padding(12.dp), color = MaterialTheme.colorScheme.error)

                        activeFile == null -> Text(
                            "Select a file to view or download",
                            Modifier.padding(16.dp),
                            style = MaterialTheme.typography.bodySmall,
                        )

                        else -> Column(
                            Modifier
                                .fillMaxSize()
                                .verticalScroll(rememberScrollState())
                                .padding(12.dp),
                        ) {
                            Text(activeFile!!.path, style = MaterialTheme.typography.labelMedium)
                            Text(
                                renderContent(activeFile!!),
                                fontFamily = FontFamily.Monospace,
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun WorkspaceTreeNode(
    node: WorkspaceFileNode,
    depth: Int,
    onOpen: (WorkspaceFileNode) -> Unit,
    onDownload: (WorkspaceFileNode) -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    val extension = node.name.substringAfterLast('.', "").lowercase()
    val canView = node.kind == "file" && isViewable(node.name, extension)
    val canDownload = node.kind == "file" && DOWNLOADABLE.contains(extension)

    if (node.kind == "dir") {
        Row(
            Modifier
                .fillMaxWidth()
                .clickable { open = !open }
                .padding(start = (depth * 12).dp, top = 4.dp, bottom = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(if (open) Icons.Default.FolderOpen else Icons.Default.Folder, null)
            Text(node.name, modifier = Modifier.padding(start = 6.dp))
        }
        if (open) {
            node.children.orEmpty().forEach { child ->
                WorkspaceTreeNode(child, depth + 1, onOpen, onDownload)
            }
        }
    } else {
        Row(
            Modifier
                .fillMaxWidth()
                .padding(start = (depth * 12).dp, top = 4.dp, bottom = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Default.Description, null)
            Text(
                node.name,
                modifier = Modifier
                    .weight(1f)
                    .padding(start = 6.dp)
                    .clickable(enabled = canView) { onOpen(node) },
                color = if (canView) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurface,
            )
            if (canDownload) {
                IconButton(onClick = { onDownload(node) }) {
                    Icon(Icons.Default.Download, contentDescription = "Download")
                }
            } else {
                Icon(Icons.Default.ChevronRight, null)
            }
        }
    }
}

private fun isViewable(name: String, ext: String): Boolean {
    val upper = name.uppercase()
    if (upper == "LICENSE" || upper == "README") return true
    return VIEWABLE.contains(ext)
}

private fun renderContent(file: WorkspaceFileContent): String {
    val ext = file.ext.lowercase()
    return when (ext) {
        "json" -> runCatching { JSONObject(file.content).toString(2) }.getOrDefault(file.content)
        "jsonl" -> file.content.lineSequence().filter { it.isNotBlank() }.joinToString("\n\n") { line ->
            runCatching { JSONObject(line).toString(2) }.getOrDefault(line)
        }
        else -> file.content
    }
}

private fun saveDownload(context: Context, fileName: String, file: WorkspaceFileContent) {
    val bytes = if (file.encoding == "base64") {
        Base64.getDecoder().decode(file.content)
    } else {
        file.content.toByteArray(Charsets.UTF_8)
    }

    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
        val values = ContentValues().apply {
            put(MediaStore.Downloads.DISPLAY_NAME, fileName)
            put(MediaStore.Downloads.MIME_TYPE, "application/octet-stream")
            put(MediaStore.Downloads.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS)
        }
        val uri = context.contentResolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
            ?: error("Unable to create download")
        context.contentResolver.openOutputStream(uri)?.use { it.write(bytes) }
            ?: error("Unable to write download")
    } else {
        @Suppress("DEPRECATION")
        val dir = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS)
        dir.mkdirs()
        java.io.File(dir, fileName).writeBytes(bytes)
    }
}
