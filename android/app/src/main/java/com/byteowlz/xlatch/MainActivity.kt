package com.byteowlz.xlatch

import android.Manifest
import android.graphics.BitmapFactory
import android.util.Base64
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.Alignment
import android.content.Intent
import android.os.Bundle
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.foundation.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.unit.dp
import androidx.fragment.app.FragmentActivity
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import org.json.JSONObject

class MainActivity : FragmentActivity() {
    private val model: AppModel by viewModels()
    private var scanResult: (String) -> Unit = {}
    private val scanner =
        registerForActivityResult(ScanContract()) { result -> result.contents?.let(scanResult) }
    private val notifications =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) {}

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (savedInstanceState == null) model.share(intent)
        setContent { AppTheme { AppScreen(model, this) } }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        model.share(intent)
    }

    fun scan(done: (String) -> Unit) {
        scanResult = done
        scanner.launch(
            ScanOptions()
                .setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                .setPrompt("Scan an xlatch pairing code")
                .setBeepEnabled(false)
                .setOrientationLocked(false)
        )
    }

    fun allowNotifications() {
        if (android.os.Build.VERSION.SDK_INT >= 33)
            notifications.launch(Manifest.permission.POST_NOTIFICATIONS)
    }
}

@Composable
private fun AppTheme(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    val scheme =
        if (dark)
            darkColorScheme(
                primary = Color(0xff77bd9d),
                onPrimary = Color(0xff10271b),
                background = Color(0xff181d1b),
                surface = Color(0xff181d1b),
                surfaceVariant = Color(0xff26332b),
            )
        else
            lightColorScheme(
                primary = Color(0xff246949),
                background = Color(0xfff7f9f6),
                surface = Color(0xfff7f9f6),
                surfaceVariant = Color(0xffe8eee7),
            )
    MaterialTheme(colorScheme = scheme, content = content)
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AppScreen(model: AppModel, activity: MainActivity) {
    var tab by rememberSaveableState("Actions")
    var pairing by remember { mutableStateOf(false) }
    var serverMenu by remember { mutableStateOf(false) }
    var compose by remember { mutableStateOf(false) }
    var text by remember { mutableStateOf("") }
    var grants by remember { mutableStateOf<JSONObject?>(null) }
    var recipients by remember { mutableStateOf(setOf<String>()) }
    var bootstrap by remember { mutableStateOf(false) }
    val clipboard = LocalClipboardManager.current
    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text("xlatch")
                        Text(
                            model.selected?.name ?: "Connect your server",
                            style = MaterialTheme.typography.labelMedium,
                        )
                    }
                },
                actions = {
                    TextButton(onClick = { serverMenu = true }, enabled = !model.busy) {
                        Text("Servers")
                    }
                    DropdownMenu(expanded = serverMenu, onDismissRequest = { serverMenu = false }) {
                        model.servers.forEach { server ->
                            DropdownMenuItem(
                                text = { Text(server.name) },
                                onClick = {
                                    model.select(server)
                                    serverMenu = false
                                },
                            )
                        }
                        DropdownMenuItem(
                            text = { Text("Add server") },
                            onClick = {
                                pairing = true
                                serverMenu = false
                            },
                        )
                    }
                    IconButton(onClick = { model.refresh() }, enabled = !model.busy) {
                        Icon(Icons.Outlined.Refresh, "Refresh")
                    }
                },
            )
        },
        bottomBar = {
            NavigationBar {
                listOf(
                        "Actions" to Icons.Outlined.GridView,
                        "Outbox" to Icons.Outlined.Outbox,
                        "Jobs" to Icons.Outlined.History,
                        "Settings" to Icons.Outlined.Settings,
                    )
                    .forEach { (name, icon) ->
                        NavigationBarItem(
                            selected = tab == name,
                            onClick = {
                                tab = name
                                if (name == "Outbox") model.refreshOutbox()
                            },
                            icon = { Icon(icon, name) },
                            label = { Text(name) },
                        )
                    }
            }
        },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding)) {
            if (model.busy) LinearProgressIndicator(Modifier.fillMaxWidth())
            model.error?.let { message ->
                Surface(color = MaterialTheme.colorScheme.errorContainer) {
                    Column(Modifier.padding(16.dp)) {
                        Text(message)
                        TextButton(onClick = { model.error = null }) { Text("Dismiss") }
                    }
                }
            }
            LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(20.dp),
                verticalArrangement = Arrangement.spacedBy(16.dp),
            ) {
                if (model.servers.isEmpty())
                    item {
                        Text(
                            "Send it to your own tools.",
                            style = MaterialTheme.typography.headlineMedium,
                        )
                        Spacer(Modifier.height(12.dp))
                        Text(
                            "Pair with an xlatch server to send links, text and files to its actions. Your server decides what this phone can run."
                        )
                        Spacer(Modifier.height(24.dp))
                        Button(onClick = { pairing = true }) { Text("Connect a server") }
                    }
                when (tab) {
                    "Actions" -> {
                        item {
                            Text(
                                "Send to an action",
                                style = MaterialTheme.typography.headlineSmall,
                            )
                            if (model.content.isEmpty()) {
                                Text("Share from another app, or write something here.")
                                TextButton(onClick = { compose = true }) {
                                    Text("Write or paste text")
                                }
                            } else {
                                Text(
                                    "${model.content.size} item(s) ready · ${model.content.joinToString { it.getString("mime_type") }}"
                                )
                                model.content
                                    .firstOrNull()
                                    ?.optString("text")
                                    ?.takeIf { it.isNotEmpty() }
                                    ?.let {
                                        Text(
                                            it.take(240),
                                            style = MaterialTheme.typography.bodySmall,
                                        )
                                    }
                                TextButton(onClick = model::clearContent) { Text("Clear content") }
                            }
                            if (model.enrollment?.optString("device_status") == "pending") {
                                Text("Waiting for approval from an enrolled phone.")
                                val payload = model.enrollment?.optString("pending_payload")
                                if (!payload.isNullOrBlank() && payload != "null")
                                    Text(
                                        "Compare code: ${sha256(payload.toByteArray()).take(12).uppercase()}"
                                    )
                            }
                        }
                        val visible =
                            model.actions.filter { action ->
                                action.getJSONObject("manifest").getString("id") !in
                                    model.disabled &&
                                    (model.content.isEmpty() ||
                                        model.content.all {
                                            accepts(action, it.getString("mime_type"))
                                        })
                            }
                        if (visible.isEmpty())
                            item {
                                Text(
                                    "No matching actions. Refresh after your server grants access, or check enabled actions in Settings.",
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                            }
                        items(visible) { action ->
                            val manifest = action.getJSONObject("manifest")
                            Column {
                                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                                    ActionIcon(manifest)
                                    Text(manifest.getString("title"), style = MaterialTheme.typography.titleMedium)
                                }
                                Text(
                                    manifest.getString("description"),
                                    style = MaterialTheme.typography.bodyMedium,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                                Button(
                                    onClick = {
                                        model.queue(action)
                                        tab = "Outbox"
                                    },
                                    enabled = !model.busy && model.content.isNotEmpty(),
                                ) {
                                    Text("Send")
                                }
                                HorizontalDivider()
                            }
                        }
                    }
                    "Outbox" -> {
                        item {
                            Text("Outbox", style = MaterialTheme.typography.headlineSmall)
                            Text(
                                "Content is saved on this phone until accepted. Delivery resumes when Android allows background work and the server is reachable."
                            )
                        }
                        if (model.outbox.isEmpty()) item { Text("Nothing waiting to send.") }
                        items(model.outbox, key = { it.getString("id") }) { item ->
                            val progress by Uploads.progress.collectAsState()
                            Column {
                                Text(
                                    item.getString("capability"),
                                    style = MaterialTheme.typography.titleMedium,
                                )
                                Text(item.getString("status"))
                                item
                                    .optString("error")
                                    .takeIf { it.isNotEmpty() }
                                    ?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                                progress[item.getString("id")]?.let {
                                    LinearProgressIndicator(
                                        progress = { it },
                                        modifier = Modifier.fillMaxWidth(),
                                    )
                                }
                                if (item.getString("status") == "paused")
                                    TextButton(
                                        onClick = { model.retry(item) },
                                        enabled = !model.busy,
                                    ) {
                                        Text("Retry same share")
                                    }
                                if (item.getString("status") in listOf("sent", "paused"))
                                    TextButton(
                                        onClick = { model.discard(item) },
                                        enabled = !model.busy,
                                    ) {
                                        Text("Remove from phone")
                                    }
                                HorizontalDivider()
                            }
                        }
                        item {
                            TextButton(onClick = model::refreshOutbox, enabled = !model.busy) {
                                Text("Refresh delivery status")
                            }
                            Text(
                                "Removing a local item cannot undo a job the server already accepted.",
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                    }
                    "Jobs" -> {
                        item {
                            Text("Recent jobs", style = MaterialTheme.typography.headlineSmall)
                            Text("Refresh to see current status and retrieve results.")
                        }
                        if (model.jobs.isEmpty()) item { Text("No jobs loaded for this server.") }
                        items(model.jobs) { job ->
                            Column {
                                Text(
                                    job.getString("capability_id"),
                                    style = MaterialTheme.typography.titleMedium,
                                )
                                Text(job.getString("status"))
                                Row {
                                    TextButton(
                                        onClick = { model.job(job.getString("id")) },
                                        enabled = !model.busy,
                                    ) {
                                        Text("View result")
                                    }
                                    if (job.getString("status") in listOf("queued", "running"))
                                        TextButton(
                                            onClick = { model.job(job.getString("id"), true) },
                                            enabled = !model.busy,
                                        ) {
                                            Text("Cancel job")
                                        }
                                }
                                HorizontalDivider()
                            }
                        }
                    }
                    "Settings" -> {
                        item {
                            Text(
                                "Server & permissions",
                                style = MaterialTheme.typography.headlineSmall,
                            )
                            model.selected?.let {
                                Text(it.name)
                                Text(
                                    it.urls.joinToString("\n"),
                                    style = MaterialTheme.typography.bodySmall,
                                )
                                Text(
                                    "Device: ${it.device}",
                                    style = MaterialTheme.typography.bodySmall,
                                )
                            }
                            TextButton(onClick = activity::allowNotifications) {
                                Text("Enable job notifications")
                            }
                            Text(
                                "Notifications use Android background work, not a central push service.",
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                        item {
                            Text(
                                "Actions on this phone",
                                style = MaterialTheme.typography.titleLarge,
                            )
                        }
                        items(model.actions) { action ->
                            Row(
                                Modifier.fillMaxWidth(),
                                horizontalArrangement = Arrangement.SpaceBetween,
                            ) {
                                Text(
                                    action.getJSONObject("manifest").getString("title"),
                                    modifier = Modifier.weight(1f),
                                )
                                Switch(
                                    checked =
                                        action.getJSONObject("manifest").getString("id") !in
                                            model.disabled,
                                    onCheckedChange = { model.enable(action, it) },
                                    enabled = !model.busy,
                                )
                            }
                        }
                        item {
                            Text("Protected approvals", style = MaterialTheme.typography.titleLarge)
                            Text(
                                if (model.enrollment?.optBoolean("is_approver") == true)
                                    "This phone can review enrollments, action activation and grants."
                                else
                                    "Refresh to check approval access. First-time setup requires a bootstrap code from your server and strong biometrics."
                            )
                            TextButton(onClick = { bootstrap = true }, enabled = !model.busy) {
                                Text("Set up this phone as approver")
                            }
                            model.selected?.let {
                                Text(
                                    "Server command: xlatch enrollment-bootstrap ${it.device}",
                                    style = MaterialTheme.typography.bodySmall,
                                )
                            }
                        }
                        if (model.enrollment?.optBoolean("is_approver") == true) {
                            item { Text("Devices & aliases", style = MaterialTheme.typography.titleLarge) }
                            items(model.deviceStatus?.optJSONArray("devices")?.objects().orEmpty().filter { !it.optBoolean("revoked") }, key = { it.getString("id") }) { device ->
                                DeviceManagementRow(device, model)
                            }
                            items(model.deviceStatus?.optJSONArray("pending")?.strings().orEmpty()) { payload ->
                                val change = JSONObject(payload)
                                TextButton(onClick = { model.reviewDevice(payload) }, enabled = !model.busy) {
                                    Text("Review ${change.getJSONObject("change").getString("kind")}: ${change.optString("alias").takeUnless { it == "null" || it.isBlank() } ?: change.getString("name")}")
                                }
                            }
                        }
                        items(model.pending) { payload ->
                            TextButton(
                                onClick = { model.reviewEnrollment(payload) },
                                enabled = !model.busy,
                            ) {
                                Text("Review device: ${JSONObject(payload).getString("name")}")
                            }
                        }
                        items(
                            model.catalog?.optJSONArray("capabilities")?.objects() ?: emptyList()
                        ) { action ->
                            Column {
                                Text(
                                    action.getJSONObject("manifest").getString("title"),
                                    style = MaterialTheme.typography.titleMedium,
                                )
                                Text(
                                    "${action.getString("status")} · ${action.getString("revision").take(12)}"
                                )
                                TextButton(
                                    onClick = {
                                        grants = action
                                        recipients = emptySet()
                                    },
                                    enabled = !model.busy,
                                ) {
                                    Text("Review activation & access")
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    if (pairing)
        CodeDialog(
            "Connect a server",
            "Paste the JSON from xlatch pair",
            scan = { done -> activity.scan(done) },
            onDismiss = { pairing = false },
        ) { code ->
            pairing = false
            model.pair(code, android.os.Build.MODEL)
        }
    if (bootstrap)
        CodeDialog(
            "Set up approvals",
            "Paste the bootstrap JSON for this device",
            scan = { done -> activity.scan(done) },
            onDismiss = { bootstrap = false },
        ) { code ->
            bootstrap = false
            model.bootstrap(activity, code)
        }
    if (compose)
        AlertDialog(
            onDismissRequest = { compose = false },
            title = { Text("Write or paste text") },
            text = {
                OutlinedTextField(
                    value = text,
                    onValueChange = { text = it },
                    label = { Text("Content") },
                    maxLines = 8,
                )
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        if (text.toByteArray().size > 100000) model.error = "Text exceeds 100 KB"
                        else {
                            model.compose(text)
                            compose = false
                        }
                    }
                ) {
                    Text("Choose action")
                }
            },
            dismissButton = { TextButton(onClick = { compose = false }) { Text("Cancel") } },
        )
    grants?.let { action ->
        AlertDialog(
            onDismissRequest = { grants = null },
            title = { Text("Grant this revision to") },
            text = {
                Column(Modifier.verticalScroll(rememberScrollState())) {
                    Text("Select none to activate without adding access.")
                    (model.catalog?.optJSONArray("devices")?.objects() ?: emptyList()).forEach {
                        device ->
                        Row {
                            Checkbox(
                                checked = device.getString("id") in recipients,
                                onCheckedChange = { on ->
                                    recipients =
                                        if (on) recipients + device.getString("id")
                                        else recipients - device.getString("id")
                                },
                            )
                            Text(device.getString("name"))
                        }
                    }
                }
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        model.prepare(action, recipients)
                        grants = null
                    }
                ) {
                    Text("Review exact changes")
                }
            },
            dismissButton = { TextButton(onClick = { grants = null }) { Text("Cancel") } },
        )
    }
    model.review?.let { review ->
        AlertDialog(
            onDismissRequest = { model.review = null },
            title = { Text("Review ${review.kind}") },
            text = {
                Column(Modifier.verticalScroll(rememberScrollState())) {
                    if (review.kind == "device") {
                        val details = JSONObject(review.payload)
                        val change = details.getJSONObject("change")
                        Text(details.optString("alias").takeUnless { it == "null" || it.isBlank() } ?: details.getString("name"))
                        Text(if (change.getString("kind") == "remove") "Revoke access and cancel unfinished jobs. History is retained." else "New alias: ${change.optString("alias").takeUnless { it == "null" } ?: "Use enrollment name"}")
                    }
                    Text(
                        "Compare code: ${sha256(review.payload.toByteArray()).take(12).uppercase()}"
                    )
                    Text(
                        "Approval signs this exact request. Check server, keys, execution and recipients."
                    )
                    SelectionContainer {
                        Text(
                            JSONObject(review.payload).toString(2),
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = { model.decide(activity, true) }) {
                    Text("Approve with biometrics")
                }
            },
            dismissButton = {
                TextButton(onClick = { model.decide(activity, false) }) {
                    Text("Reject with biometrics")
                }
            },
        )
    }
    model.result?.let { result ->
        AlertDialog(
            onDismissRequest = { model.result = null },
            title = { Text("Job result") },
            text = {
                SelectionContainer {
                    Text(
                        result,
                        modifier = Modifier.verticalScroll(rememberScrollState()),
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            },
            confirmButton = {
                TextButton(onClick = { clipboard.setText(AnnotatedString(result)) }) {
                    Text("Copy")
                }
            },
            dismissButton = { TextButton(onClick = { model.result = null }) { Text("Close") } },
        )
    }
}

@Composable
private fun rememberSaveableState(initial: String) =
    androidx.compose.runtime.saveable.rememberSaveable { mutableStateOf(initial) }

@Composable
private fun CodeDialog(
    title: String,
    hint: String,
    scan: ((String) -> Unit) -> Unit,
    onDismiss: () -> Unit,
    submit: (String) -> Unit,
) {
    var code by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            Column {
                OutlinedTextField(
                    value = code,
                    onValueChange = { code = it },
                    label = { Text(hint) },
                    maxLines = 6,
                )
                TextButton(onClick = { scan { code = it } }) { Text("Scan QR code") }
            }
        },
        confirmButton = {
            TextButton(onClick = { submit(code) }, enabled = code.isNotBlank()) { Text("Continue") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}


@Composable
private fun ActionIcon(manifest: JSONObject) {
    val encoded = manifest.optJSONObject("icon")?.optString("png_base64")
    val bitmap = remember(encoded) {
        runCatching {
            require(encoded != null && encoded.length <= 175000)
            val bytes = Base64.decode(encoded, Base64.DEFAULT)
            require(bytes.size <= 131072)
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
            require(bounds.outWidth in 1..256 && bounds.outHeight in 1..256)
            BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap()
        }.getOrNull()
    }
    if (bitmap != null) Image(bitmap, contentDescription = null, modifier = Modifier.size(32.dp))
    else Icon(Icons.Outlined.Bolt, contentDescription = null, modifier = Modifier.size(32.dp), tint = MaterialTheme.colorScheme.primary)
}

@Composable
private fun DeviceManagementRow(device: JSONObject, model: AppModel) {
    val id = device.getString("id")
    val initialAlias = device.optString("alias").takeUnless { it == "null" }.orEmpty()
    var alias by remember(id, initialAlias) { mutableStateOf(initialAlias) }
    Column(Modifier.fillMaxWidth()) {
        Text(initialAlias.ifEmpty { device.getString("name") }, style = MaterialTheme.typography.titleMedium)
        Text("${device.getString("name")} · $id", style = MaterialTheme.typography.bodySmall)
        OutlinedTextField(value = alias, onValueChange = { alias = it }, label = { Text("Custom alias") }, singleLine = true)
        Row {
            TextButton(onClick = { model.prepareDevice(id, JSONObject().put("kind", "alias").put("alias", alias.ifEmpty { null } ?: JSONObject.NULL)) }, enabled = !model.busy) { Text("Review alias") }
            TextButton(onClick = { model.prepareDevice(id, JSONObject().put("kind", "remove")) }, enabled = !model.busy && id != model.selected?.device) { Text("Review removal") }
        }
    }
}
