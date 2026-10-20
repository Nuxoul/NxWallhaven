package io.github.nuxoul.nxwallhaven.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.nuxoul.nxwallhaven.NxViewModel

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun SettingsScreen(vm: NxViewModel) {
    val state by vm.ui.collectAsStateWithLifecycle()
    // Never put the API key into saved instance state or echo a stored key.
    var keyInput by remember { mutableStateOf("") }
    var showKey by remember { mutableStateOf(false) }
    var allowNsfw by remember(state.prefs.allowNsfw, state.prefs.hasKey) { mutableStateOf(state.prefs.allowNsfw && state.prefs.hasKey) }
    var theme by remember(state.prefs.theme) { mutableIntStateOf(state.prefs.theme) }
    var equalGrid by remember(state.prefs.grid) { mutableStateOf(state.prefs.grid) }
    var confirmClear by rememberSaveable { mutableStateOf(false) }
    var confirmDelete by rememberSaveable { mutableStateOf(false) }
    LaunchedEffect(Unit) { vm.refreshCache() }
    Scaffold(
        modifier = Modifier.fillMaxSize().testTag("settings"),
        containerColor = MaterialTheme.colorScheme.background,
        topBar = { TopAppBar(title = { Text("设置") }) },
        bottomBar = { AppNavigation(true, vm) }
    ) { padding ->
        LazyColumn(
            modifier = Modifier.fillMaxSize().padding(padding),
            contentPadding = PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(16.dp)
        ) {
            item {
                SettingsPanel("账户与内容") {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Icon(Icons.Outlined.Key, contentDescription = null, tint = MaterialTheme.colorScheme.primary)
                        Text(if (state.prefs.hasKey) "已保存 API 密钥" else "未保存 API 密钥", style = MaterialTheme.typography.titleMedium)
                    }
                    Text("浏览 SFW 内容无需密钥。输入框留空可保留已保存的密钥。",
                        style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    OutlinedTextField(
                        value = keyInput, onValueChange = { keyInput = it.trim(); vm.keyInputChanged() }, singleLine = true,
                        label = { Text(if (state.prefs.hasKey) "更换 API 密钥" else "Wallhaven API 密钥") },
                        placeholder = { Text("留空以保留已保存的密钥") },
                        visualTransformation = if (showKey) VisualTransformation.None else PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                        trailingIcon = { NamedIcon(if (showKey) Icons.Outlined.VisibilityOff else Icons.Outlined.Visibility,
                            if (showKey) "隐藏 API 密钥" else "显示 API 密钥", { showKey = !showKey }) },
                        modifier = Modifier.fillMaxWidth().testTag("api-key")
                    )
                    OutlinedButton(onClick = { vm.verifyKey(keyInput) }, enabled = !state.keyChecking && (keyInput.isNotBlank() || state.prefs.hasKey),
                        modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag("verify-api-key")) {
                        Text(if(state.keyChecking)"正在验证…" else "验证 API 密钥")
                    }
                    Text(state.keyStatus,modifier=Modifier.testTag("api-key-status"),style=MaterialTheme.typography.bodySmall)
                    Text("只读请求 API /settings，不修改账户。API 密钥验证不是网站登录，也不验证 Cookie。",
                        style=MaterialTheme.typography.bodySmall,color=MaterialTheme.colorScheme.onSurfaceVariant)
                    if (state.prefs.hasKey) TextButton(onClick = { confirmDelete = true },
                        modifier = Modifier.heightIn(min = 48.dp).testTag("delete-key")) {
                        Icon(Icons.Outlined.DeleteOutline, contentDescription = null)
                        Spacer(Modifier.width(8.dp)); Text("删除已保存的密钥", color = MaterialTheme.colorScheme.error)
                    }
                    HorizontalDivider()
                    SwitchSetting(
                        title = "允许 NSFW 内容",
                        subtitle = if (state.prefs.hasKey) "默认关闭。开启后可选择 NSFW 分级，但不会自动选中。"
                            else "请先保存 API 密钥。",
                        checked = allowNsfw, enabled = state.prefs.hasKey,
                        onChange = { allowNsfw = it }, tag = "allow-nsfw"
                    )
                }
            }
            item {
                SettingsPanel("外观") {
                    ChoiceMenu("主题", theme.toString(), listOf("0" to "跟随系统", "1" to "深色", "2" to "浅色")) {
                        theme = it.toIntOrNull() ?: 0
                    }
                    SwitchSetting("等大网格", "使用方形卡片并完整显示缩略图；关闭后按图片原比例排列。",
                        equalGrid, onChange = { equalGrid = it }, tag = "equal-grid")
                    Text("竖屏 2 列 · 横屏 3 列", style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
            item {
                Button(
                    onClick = {
                        vm.saveSettings(keyInput, allowNsfw && state.prefs.hasKey, theme, equalGrid)
                        keyInput = ""; showKey = false
                    }, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag("save-settings")
                ) {
                    Icon(Icons.Outlined.Check, contentDescription = null)
                    Spacer(Modifier.width(8.dp)); Text("保存设置")
                }
            }
            item {
                SettingsPanel("磁盘缓存") {
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text(formatBytes(state.cache.bytes), style = MaterialTheme.typography.headlineSmall)
                            Text("${state.cache.bytes} 字节 · ${state.cache.files} 个文件", style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.testTag("cache-bytes"))
                        }
                        NamedIcon(Icons.Outlined.Refresh, "刷新缓存用量", { vm.refreshCache() }, enabled = !state.cacheBusy)
                    }
                    if (state.cacheBusy) LinearProgressIndicator(Modifier.fillMaxWidth())
                    Text("缓存目录", style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    SelectionContainer {
                        Text(state.cache.path.ifBlank { "缓存位置暂不可用" }, style = MaterialTheme.typography.bodySmall,
                            modifier = Modifier.testTag("cache-path"))
                    }
                    Text("图片会先缓存到本地。查看原图时，会从本地文件按需加载完整原图分块。",
                        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    OutlinedButton(onClick = { confirmClear = true }, enabled = !state.cacheBusy && state.cache.bytes > 0,
                        modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag("clear-cache")) {
                        Icon(Icons.Outlined.DeleteSweep, contentDescription = null)
                        Spacer(Modifier.width(8.dp)); Text("清除磁盘缓存")
                    }
                }
            }
            item {
                Text("NxWallhaven\n浏览、保存和设置你喜欢的 Wallhaven 壁纸。",
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 8.dp, vertical = 12.dp))
            }
        }
    }
    if (confirmClear) AlertDialog(
        onDismissRequest = { confirmClear = false }, title = { Text("清除磁盘缓存？") },
        text = { Text("将清除 ${formatBytes(state.cache.bytes)} 的缓存图片，之后需要重新加载。已下载到相册的文件不会删除，偏好设置和 API 密钥也不会更改。") },
        confirmButton = { TextButton(onClick = { confirmClear = false; vm.clearCache() }, Modifier.heightIn(min = 48.dp).testTag("confirm-clear-cache")) { Text("清除缓存") } },
        dismissButton = { TextButton(onClick = { confirmClear = false }, Modifier.heightIn(min = 48.dp)) { Text("取消") } }
    )
    if (confirmDelete) AlertDialog(
        onDismissRequest = { confirmDelete = false }, title = { Text("删除已保存的 API 密钥？") },
        text = { Text("将删除已保存的密钥并关闭 NSFW 权限，仍可继续浏览 SFW 内容。") },
        confirmButton = { TextButton(onClick = { confirmDelete = false; keyInput = ""; allowNsfw = false; vm.clearKey() },
            modifier = Modifier.heightIn(min = 48.dp).testTag("confirm-delete-key")) { Text("删除密钥") } },
        dismissButton = { TextButton(onClick = { confirmDelete = false }, Modifier.heightIn(min = 48.dp)) { Text("取消") } }
    )
}

@Composable
private fun SettingsPanel(title: String, content: @Composable ColumnScope.() -> Unit) {
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface), modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(18.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Text(title, style = MaterialTheme.typography.titleLarge)
            content()
        }
    }
}

@Composable
private fun SwitchSetting(
    title: String, subtitle: String, checked: Boolean, enabled: Boolean = true,
    onChange: (Boolean) -> Unit, tag: String
) {
    Row(
        Modifier.fillMaxWidth().heightIn(min = 56.dp).clickable(enabled = enabled, role = Role.Switch) { onChange(!checked) }.testTag(tag),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)
    ) {
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.titleMedium)
            Text(subtitle, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Switch(checked = checked, enabled = enabled, onCheckedChange = onChange)
    }
}
