package io.github.nuxoul.nxwallhaven.ui

import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.staggeredgrid.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import coil3.compose.AsyncImage
import com.davemorrissey.labs.subscaleview.ImageSource
import com.davemorrissey.labs.subscaleview.SubsamplingScaleImageView
import io.github.nuxoul.nxwallhaven.NxViewModel
import io.github.nuxoul.nxwallhaven.shared.DownloadItem

/** Local records and local URIs only; acquisition and platform actions belong to the controller. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun LocalScreen(vm: NxViewModel) {
    val controller = vm.local
    val state by controller.ui.collectAsStateWithLifecycle()
    val app by vm.ui.collectAsStateWithLifecycle()
    val nsfwAllowed = app.prefs.allowNsfw && app.prefs.hasKey
    val tasks = state.tasks.filter { nsfwAllowed || !it.wall.purity.equals("nsfw", ignoreCase = true) }
    val library = state.library.filter { nsfwAllowed || !it.wall.purity.equals("nsfw", ignoreCase = true) }
    var filtersOpen by rememberSaveable { mutableStateOf(false) }
    var deleteOpen by rememberSaveable { mutableStateOf(false) }
    var cancelId by rememberSaveable { mutableStateOf<String?>(null) }
    val viewer = state.viewer?.takeIf {
        (nsfwAllowed || !it.wall.purity.equals("nsfw", ignoreCase = true)) && localReadableUri(it) != null
    }

    DisposableEffect(controller) { controller.start(); onDispose { controller.stop() } }
    LaunchedEffect(nsfwAllowed) { if (!nsfwAllowed) controller.clearSelection() }
    LaunchedEffect(state.viewer?.id, state.viewer?.uri, state.viewer?.state, nsfwAllowed) {
        if (state.viewer != null && viewer == null) controller.closeViewer()
    }
    BackHandler(enabled = viewer == null && state.selected.isNotEmpty()) {
        deleteOpen = false
        controller.clearSelection()
    }

    Box(Modifier.fillMaxSize().testTag("local")) {
        Scaffold(
            containerColor = MaterialTheme.colorScheme.background,
            bottomBar = { AppNavigation(false, vm) },
            topBar = {
                Column {
                    TopAppBar(
                        title = { Text("本地") },
                        actions = {
                            if (state.tab == 1) NamedIcon(Icons.Outlined.Tune, "筛选本地图库", { filtersOpen = true })
                            NamedIcon(Icons.Outlined.Refresh, "刷新本地记录", { controller.refresh() }, enabled = !state.busy)
                        }
                    )
                    TabRow(selectedTabIndex = state.tab.coerceIn(0, 1)) {
                        Tab(selected = state.tab == 0, onClick = { controller.clearSelection(); controller.tab(0) },
                            text = { Text("下载") }, modifier = Modifier.heightIn(min = 48.dp).testTag("local-tab-downloads"))
                        Tab(selected = state.tab == 1, onClick = { controller.clearSelection(); controller.tab(1) },
                            text = { Text("图库") }, modifier = Modifier.heightIn(min = 48.dp).testTag("local-tab-library"))
                    }
                }
            }
        ) { padding ->
            Column(Modifier.fillMaxSize().padding(padding)) {
                if (state.busy) LinearProgressIndicator(Modifier.fillMaxWidth())
                state.message?.let { message ->
                    Surface(color = MaterialTheme.colorScheme.surfaceContainerHigh) {
                        Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                            Text(message, Modifier.weight(1f).padding(vertical = 10.dp), style = MaterialTheme.typography.bodySmall)
                            TextButton(onClick = { controller.refresh() }, enabled = !state.busy,
                                modifier = Modifier.heightIn(min = 48.dp)) { Text("重试") }
                        }
                    }
                }
                if (state.tab == 0) {
                    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text("下载任务 · ${tasks.size}", style = MaterialTheme.typography.titleSmall)
                            Text("仅清理失败/取消记录，保留图库与文件", style = MaterialTheme.typography.labelSmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                        TextButton(onClick = { controller.clearFinished() },
                            enabled = !state.busy && tasks.any { it.state in setOf("cancelled", "failed") },
                            modifier = Modifier.heightIn(min = 48.dp)) { Text("清理记录") }
                    }
                    if (tasks.isEmpty()) {
                        LocalEmpty("暂无下载任务", "下载的进度与操作会显示在这里。", state.busy, { controller.refresh() }, Modifier.weight(1f))
                    } else {
                        LazyColumn(Modifier.weight(1f).fillMaxWidth(), contentPadding = PaddingValues(12.dp),
                            verticalArrangement = Arrangement.spacedBy(10.dp)) {
                            items(tasks, key = { it.id }) { item ->
                                LocalQueueRow(item,
                                    onPause = { controller.pause(item.id) }, onResume = { controller.resume(item.id) },
                                    onCancel = { cancelId = item.id }, onRetry = { controller.retry(item.id) },
                                    onRefresh = { controller.refresh() },
                                    onOpen = { controller.tab(1); controller.open(item) })
                            }
                        }
                    }
                } else {
                    Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                        if (state.selected.isNotEmpty()) {
                            NamedIcon(Icons.Outlined.Close, "取消选择", { controller.clearSelection() })
                        }
                        Text(if (state.selected.isEmpty()) "${library.size} 张本地图片" else "已选择 ${state.selected.size} 项",
                            Modifier.weight(1f), style = MaterialTheme.typography.titleSmall)
                        TextButton(onClick = {
                            library.filterNot { it.id in state.selected }.forEach { controller.select(it.id) }
                        }, enabled = library.isNotEmpty(), modifier = Modifier.heightIn(min = 48.dp)) { Text("全选") }
                        if (state.selected.isNotEmpty()) {
                            NamedIcon(Icons.Outlined.DeleteOutline, "删除所选本地图片", { deleteOpen = true },
                                Modifier.testTag("delete-local-selected"), enabled = !state.busy)
                        }
                    }
                    if (library.isEmpty()) {
                        LocalEmpty("图库中没有图片", "保存的原图可离线查看；也可尝试放宽本地筛选。", state.busy,
                            { controller.refresh() }, Modifier.weight(1f))
                    } else {
                        val columns = if (LocalConfiguration.current.screenWidthDp >= 600) 3 else 2
                        LazyVerticalStaggeredGrid(
                            columns = StaggeredGridCells.Fixed(columns), modifier = Modifier.weight(1f).fillMaxWidth(),
                            contentPadding = PaddingValues(start = 12.dp, end = 12.dp, top = 4.dp, bottom = 24.dp),
                            horizontalArrangement = Arrangement.spacedBy(10.dp), verticalItemSpacing = 10.dp
                        ) {
                            items(library, key = { it.id }) { item ->
                                LocalLibraryCard(item, item.id in state.selected,
                                    onSelect = { controller.select(item.id) },
                                    onClick = {
                                        if (state.selected.isNotEmpty()) controller.select(item.id)
                                        else if (localReadableUri(item) != null) controller.open(item)
                                    })
                            }
                        }
                    }
                }
            }
        }
        // Kept inside this destination: returning never replaces the local grid or starts a remote detail request.
        if (viewer != null) key(viewer.id, viewer.uri) {
            LocalOriginalViewer(viewer, state.busy, state.message,
                onClose = { controller.closeViewer() }, onShare = { controller.share(viewer) },
                onExport = { controller.export(viewer) }, onWallpaper = { controller.wallpaper(viewer, it) })
        }
    }

    if (filtersOpen) LocalFilterSheet(state.query, state.sort, state.purity, state.minWidth, nsfwAllowed,
        onDismiss = { filtersOpen = false }, onApply = { query, sort, purity, width ->
            filtersOpen = false
            controller.clearSelection()
            controller.filter(query, sort, purity, width)
        })

    val cancelItem = tasks.firstOrNull { it.id == cancelId && it.state in setOf("queued", "running", "paused") }
    if (cancelItem != null) AlertDialog(
        onDismissRequest = { cancelId = null }, title = { Text("取消下载？") },
        text = { Text("将停止任务 ${cancelItem.id}。不会清理其他已保存的图片。") },
        confirmButton = { TextButton(onClick = { cancelId = null; controller.cancel(cancelItem.id) },
            modifier = Modifier.heightIn(min = 48.dp)) { Text("取消下载") } },
        dismissButton = { TextButton(onClick = { cancelId = null }, modifier = Modifier.heightIn(min = 48.dp)) { Text("保留任务") } }
    )
    if (deleteOpen && state.selected.isNotEmpty()) AlertDialog(
        onDismissRequest = { deleteOpen = false }, title = { Text("处理所选的 ${state.selected.size} 项？") },
        text = { Text("删除文件会从设备移除原图，无法在本应用撤销。系统可能要求额外的删除授权，请确认系统提示。\n\n仅移除记录会保留设备中的文件。") },
        confirmButton = {
            Column {
                TextButton(onClick = { deleteOpen = false; controller.deleteSelected(true) },
                    enabled = !state.busy, modifier = Modifier.heightIn(min = 48.dp).testTag("confirm-delete-local-files")) {
                    Text("删除文件及记录", color = MaterialTheme.colorScheme.error)
                }
                TextButton(onClick = { deleteOpen = false; controller.deleteSelected(false) },
                    enabled = !state.busy, modifier = Modifier.heightIn(min = 48.dp).testTag("confirm-remove-local-records")) {
                    Text("仅移除记录，保留文件")
                }
            }
        },
        dismissButton = { TextButton(onClick = { deleteOpen = false }, modifier = Modifier.heightIn(min = 48.dp)) { Text("取消") } }
    )
}

/** A remote URL can never reach Coil or the original viewer through this screen. */
private fun localReadableUri(item: DownloadItem): String? = item.uri?.takeIf { value ->
    item.state != "missing" && value.isNotBlank() &&
        runCatching { Uri.parse(value).scheme?.lowercase() in setOf("content", "file") }.getOrDefault(false)
}

private fun localStatus(state: String): String = when (state) {
    "queued" -> "等待下载"
    "running" -> "正在下载"
    "paused" -> "已暂停"
    "cancelled" -> "已取消"
    "failed" -> "下载失败"
    "completed" -> "已完成"
    "missing" -> "文件缺失"
    else -> "未知状态"
}

@Composable
private fun LocalEmpty(title: String, explanation: String, busy: Boolean, onRefresh: () -> Unit, modifier: Modifier) {
    Column(modifier.fillMaxWidth().padding(24.dp), verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally) {
        Icon(Icons.Outlined.Photo, contentDescription = null, modifier = Modifier.size(44.dp),
            tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Spacer(Modifier.height(16.dp))
        Text(title, style = MaterialTheme.typography.titleMedium)
        Text(explanation, Modifier.padding(vertical = 12.dp), style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant)
        OutlinedButton(onClick = onRefresh, enabled = !busy, modifier = Modifier.heightIn(min = 48.dp)) { Text("刷新 / 重试") }
    }
}

@Composable
private fun LocalQueueRow(
    item: DownloadItem, onPause: () -> Unit, onResume: () -> Unit, onCancel: () -> Unit,
    onRetry: () -> Unit, onRefresh: () -> Unit, onOpen: () -> Unit
) {
    Card(Modifier.fillMaxWidth().testTag("queue-${item.id}"), shape = RoundedCornerShape(16.dp),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface)) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                LocalThumbnail(item, Modifier.size(64.dp))
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                    Text(item.id, style = MaterialTheme.typography.titleSmall)
                    Text("${item.wall.resolution} · ${localStatus(item.state)}", style = MaterialTheme.typography.bodySmall,
                        color = if (item.state == "failed" || item.state == "missing") MaterialTheme.colorScheme.error
                        else MaterialTheme.colorScheme.onSurfaceVariant)
                    val total = item.total?.takeIf { it > 0 }
                    Text(if (total != null) "${formatBytes(item.received)} / ${formatBytes(total)}"
                        else "已接收 ${formatBytes(item.received)} · 总大小未知", style = MaterialTheme.typography.labelSmall)
                }
            }
            if (item.state in setOf("queued", "running", "paused")) {
                val progress = item.percent
                if (progress != null) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        LinearProgressIndicator(progress = { progress }, modifier = Modifier.weight(1f))
                        Text("${(progress * 100).toInt()}%", style = MaterialTheme.typography.labelSmall)
                    }
                } else if (item.state == "running") LinearProgressIndicator(Modifier.fillMaxWidth())
            }
            item.error?.takeIf { it.isNotBlank() }?.let {
                Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
            }
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End, verticalAlignment = Alignment.CenterVertically) {
                when (item.state) {
                    "queued", "running" -> {
                        NamedIcon(Icons.Outlined.Pause, "暂停下载 ${item.id}", onPause, Modifier.testTag("queue-pause-${item.id}"))
                        NamedIcon(Icons.Outlined.Close, "取消下载 ${item.id}", onCancel, Modifier.testTag("queue-cancel-${item.id}"))
                    }
                    "paused" -> {
                        NamedIcon(Icons.Outlined.PlayArrow, "继续下载 ${item.id}", onResume, Modifier.testTag("queue-resume-${item.id}"))
                        NamedIcon(Icons.Outlined.Close, "取消下载 ${item.id}", onCancel, Modifier.testTag("queue-cancel-${item.id}"))
                    }
                    "failed", "cancelled" -> TextButton(onClick = onRetry,
                        modifier = Modifier.heightIn(min = 48.dp).testTag("queue-retry-${item.id}")) { Text("重试下载") }
                    "completed" -> TextButton(onClick = onOpen, enabled = localReadableUri(item) != null,
                        modifier = Modifier.heightIn(min = 48.dp)) { Text("在图库中查看") }
                    "missing" -> TextButton(onClick = onRefresh, modifier = Modifier.heightIn(min = 48.dp)) { Text("刷新文件状态") }
                }
            }
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun LocalLibraryCard(item: DownloadItem, selected: Boolean, onSelect: () -> Unit, onClick: () -> Unit) {
    Card(
        modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag("library-card-${item.id}")
            .combinedClickable(onClickLabel = "查看本地原图或选择图片", onLongClickLabel = "选择此图片",
                onClick = onClick, onLongClick = onSelect)
            .semantics { contentDescription = "本地壁纸 ${item.id}，${item.wall.resolution}，${localStatus(item.state)}" },
        shape = RoundedCornerShape(12.dp),
        border = if (selected) BorderStroke(2.dp, MaterialTheme.colorScheme.primary) else null,
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant)
    ) {
        Box(Modifier.fillMaxWidth().aspectRatio(item.wall.aspect.coerceAtLeast(0.01f))) {
            LocalThumbnail(item, Modifier.fillMaxSize())
            Surface(color = MaterialTheme.colorScheme.surface.copy(alpha = 0.9f), shape = RoundedCornerShape(bottomStart = 12.dp),
                modifier = Modifier.align(Alignment.TopEnd)) {
                Checkbox(checked = selected, onCheckedChange = { onSelect() },
                    modifier = Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp)
                        .semantics { contentDescription = "选择本地壁纸 ${item.id}" })
            }
            Surface(color = MaterialTheme.colorScheme.surface.copy(alpha = 0.88f), shape = RoundedCornerShape(topStart = 8.dp),
                modifier = Modifier.align(Alignment.BottomEnd)) {
                Text(item.wall.resolution, Modifier.padding(horizontal = 8.dp, vertical = 4.dp), style = MaterialTheme.typography.labelSmall)
            }
        }
    }
}

@Composable
private fun LocalThumbnail(item: DownloadItem, modifier: Modifier) {
    val uri = localReadableUri(item)
    var failed by remember(uri, item.updated) { mutableStateOf(false) }
    Box(modifier.background(MaterialTheme.colorScheme.surfaceVariant), contentAlignment = Alignment.Center) {
        if (uri != null && !failed) {
            AsyncImage(model = uri, contentDescription = "本地壁纸 ${item.id}", contentScale = ContentScale.Fit,
                modifier = Modifier.fillMaxSize(), onError = { failed = true })
        } else {
            Column(Modifier.padding(6.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                Icon(if (item.state == "missing" || failed) Icons.Outlined.BrokenImage else Icons.Outlined.Photo,
                    contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant)
                if (item.state == "missing" || failed) Text("文件不可用", style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun LocalFilterSheet(
    initialQuery: String, initialSort: Int, initialPurity: String, initialWidth: Int, nsfwAllowed: Boolean,
    onDismiss: () -> Unit, onApply: (String, Int, String, Int) -> Unit
) {
    var query by rememberSaveable { mutableStateOf(initialQuery) }
    var sort by rememberSaveable { mutableIntStateOf(initialSort.coerceIn(0, 2)) }
    var purity by rememberSaveable { mutableStateOf(if (initialPurity == "nsfw" && !nsfwAllowed) "all" else initialPurity) }
    var width by rememberSaveable { mutableIntStateOf(initialWidth) }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(Modifier.fillMaxWidth().imePadding().verticalScroll(rememberScrollState()).padding(24.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp)) {
            Text("筛选本地图库", style = MaterialTheme.typography.headlineSmall)
            Text("仅筛选本地记录，不请求远程图片或详情。", style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant)
            OutlinedTextField(value = query, onValueChange = { query = it }, singleLine = true,
                label = { Text("搜索本地记录") }, placeholder = { Text("图片 ID、标签或作者") },
                modifier = Modifier.fillMaxWidth().testTag("local-query"))
            LocalFilterDropdown("排序", sort.toString(), listOf("0" to "最新保存", "1" to "最早保存", "2" to "文件大小"),
                onChoose = { sort = it.toInt() })
            LocalFilterDropdown("内容分级", purity,
                listOf("all" to "全部可见内容", "sfw" to "SFW", "sketchy" to "Sketchy", "nsfw" to "NSFW"),
                enabledOption = { it != "nsfw" || nsfwAllowed }, onChoose = { purity = it })
            if (!nsfwAllowed) Text("NSFW 需先配置 API Key 并在设置中允许。", style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant)
            LocalFilterDropdown("最小宽度", width.toString(), listOf("0" to "不限", "1920" to "1920 px", "3840" to "3840 px"),
                onChoose = { width = it.toInt() })
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                OutlinedButton(onClick = { query = ""; sort = 0; purity = "all"; width = 0 }, modifier = Modifier.heightIn(min = 48.dp)) { Text("重置") }
                Button(onClick = { onApply(query.trim(), sort, if (purity == "nsfw" && !nsfwAllowed) "all" else purity, width) },
                    modifier = Modifier.weight(1f).heightIn(min = 48.dp)) { Text("应用筛选") }
            }
            Spacer(Modifier.height(8.dp))
        }
    }
}

@Composable
private fun LocalFilterDropdown(
    label: String, value: String, options: List<Pair<String, String>>,
    enabledOption: (String) -> Boolean = { true }, onChoose: (String) -> Unit
) {
    var expanded by remember { mutableStateOf(false) }
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(label, style = MaterialTheme.typography.labelLarge)
        Box {
            OutlinedButton(onClick = { expanded = true }, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) {
                Text(options.firstOrNull { it.first == value }?.second ?: value, Modifier.weight(1f))
                Icon(Icons.Outlined.ArrowDropDown, contentDescription = null)
            }
            DropdownMenu(expanded = expanded, onDismissRequest = { expanded = false }) {
                options.forEach { (key, title) ->
                    DropdownMenuItem(text = { Text(title) }, enabled = enabledOption(key),
                        onClick = { expanded = false; onChoose(key) }, modifier = Modifier.heightIn(min = 48.dp))
                }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun LocalOriginalViewer(
    item: DownloadItem, busy: Boolean, message: String?, onClose: () -> Unit, onShare: () -> Unit,
    onExport: () -> Unit, onWallpaper: (Boolean) -> Unit
) {
    var metadata by rememberSaveable { mutableStateOf(false) }
    var wallpaperMenu by remember { mutableStateOf(false) }
    var wallpaperTarget by rememberSaveable { mutableStateOf<Boolean?>(null) }
    var dimensions by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var attempt by remember { mutableIntStateOf(0) }
    BackHandler { onClose() }
    Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        Scaffold(
            containerColor = MaterialTheme.colorScheme.background,
            topBar = {
                TopAppBar(title = {
                    Column {
                        Text("本地原图", style = MaterialTheme.typography.titleMedium)
                        Text("${item.id} · ${item.wall.resolution}", style = MaterialTheme.typography.bodySmall,
                            maxLines = 1, overflow = TextOverflow.Ellipsis, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }, navigationIcon = {
                    NamedIcon(Icons.AutoMirrored.Outlined.ArrowBack, "返回本地图库", onClose, Modifier.testTag("local-back"))
                }, actions = { NamedIcon(Icons.Outlined.Info, "本地图片信息", { metadata = true }) })
            },
            bottomBar = {
                Surface(color = MaterialTheme.colorScheme.surface) {
                    Column(Modifier.navigationBarsPadding().padding(horizontal = 12.dp, vertical = 8.dp)) {
                        if (busy) LinearProgressIndicator(Modifier.fillMaxWidth())
                        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                            TextButton(onClick = onShare, enabled = !busy && error == null,
                                modifier = Modifier.weight(1f).heightIn(min = 48.dp)) {
                                Icon(Icons.Outlined.Share, contentDescription = null)
                                Spacer(Modifier.width(6.dp)); Text("分享")
                            }
                            TextButton(onClick = onExport, enabled = !busy && error == null,
                                modifier = Modifier.weight(1f).heightIn(min = 48.dp)) {
                                Icon(Icons.Outlined.FolderOpen, contentDescription = null)
                                Spacer(Modifier.width(6.dp)); Text("导出")
                            }
                            Box(Modifier.weight(1f)) {
                                TextButton(onClick = { wallpaperMenu = true }, enabled = !busy && error == null,
                                    modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) {
                                    Icon(Icons.Outlined.Wallpaper, contentDescription = null)
                                    Spacer(Modifier.width(6.dp)); Text("壁纸")
                                }
                                DropdownMenu(expanded = wallpaperMenu, onDismissRequest = { wallpaperMenu = false }) {
                                    DropdownMenuItem(text = { Text("设为主屏幕壁纸") },
                                        onClick = { wallpaperMenu = false; wallpaperTarget = false }, modifier = Modifier.heightIn(min = 48.dp))
                                    DropdownMenuItem(text = { Text("设为锁屏壁纸") },
                                        onClick = { wallpaperMenu = false; wallpaperTarget = true }, modifier = Modifier.heightIn(min = 48.dp))
                                }
                            }
                        }
                    }
                }
            }
        ) { padding ->
            Column(Modifier.fillMaxSize().padding(padding)) {
                message?.let {
                    Text(it, Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceContainerHigh).padding(12.dp),
                        style = MaterialTheme.typography.bodySmall)
                }
                Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
                    key(attempt) {
                        LocalTiledOriginal(item, onLoaded = { dimensions = it }, onError = { error = it },
                            modifier = Modifier.fillMaxSize().testTag("local-original"))
                    }
                    if (error != null) {
                        Surface(color = MaterialTheme.colorScheme.surface.copy(alpha = 0.96f), shape = MaterialTheme.shapes.large) {
                            Column(Modifier.padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally,
                                verticalArrangement = Arrangement.spacedBy(12.dp)) {
                                Icon(Icons.Outlined.BrokenImage, contentDescription = null, modifier = Modifier.size(40.dp))
                                Text("无法显示本地原图", style = MaterialTheme.typography.titleMedium)
                                Text(error!!, color = MaterialTheme.colorScheme.error)
                                OutlinedButton(onClick = { error = null; dimensions = null; attempt++ }, modifier = Modifier.heightIn(min = 48.dp)) { Text("重试读取本地文件") }
                            }
                        }
                    } else if (dimensions == null) {
                        Surface(color = MaterialTheme.colorScheme.surface.copy(alpha = 0.94f), shape = MaterialTheme.shapes.large) {
                            Column(Modifier.padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally,
                                verticalArrangement = Arrangement.spacedBy(12.dp)) {
                                CircularProgressIndicator(Modifier.size(28.dp))
                                Text("正在读取本地原图…")
                            }
                        }
                    }
                }
                Text(dimensions?.let { "完整原图 · $it · 双指缩放，最高 8×" } ?: "本地分块读取 · 不访问网络",
                    Modifier.padding(horizontal = 16.dp, vertical = 10.dp), style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
    if (metadata) LocalMetadataSheet(item, onDismiss = { metadata = false })
    wallpaperTarget?.let { lock ->
        AlertDialog(onDismissRequest = { wallpaperTarget = null },
            title = { Text(if (lock) "设为锁屏壁纸？" else "设为主屏幕壁纸？") },
            text = { Text("将使用当前本地原图替换${if (lock) "锁屏" else "主屏幕"}壁纸；系统可能要求额外确认。") },
            confirmButton = { TextButton(onClick = { wallpaperTarget = null; onWallpaper(lock) }, enabled = !busy,
                modifier = Modifier.heightIn(min = 48.dp)) { Text("确认设置") } },
            dismissButton = { TextButton(onClick = { wallpaperTarget = null }, modifier = Modifier.heightIn(min = 48.dp)) { Text("取消") } })
    }
}

/** Full-resolution region decoding with EXIF orientation; no thumbnail or remote fallback. */
@Composable
private fun LocalTiledOriginal(item: DownloadItem, onLoaded: (String) -> Unit, onError: (String) -> Unit, modifier: Modifier) {
    val currentLoaded by rememberUpdatedState(onLoaded)
    val currentError by rememberUpdatedState(onError)
    AndroidView(
        factory = { context ->
            SubsamplingScaleImageView(context).apply {
                contentDescription = "完整本地原图。双指缩放或双击放大。"
                setOrientation(SubsamplingScaleImageView.ORIENTATION_USE_EXIF)
                setMinimumScaleType(SubsamplingScaleImageView.SCALE_TYPE_CENTER_INSIDE)
                setMaxScale(8f)
                setMaxTileSize(2048)
                setMinimumTileDpi(160)
                setPanEnabled(true)
                setZoomEnabled(true)
                setOnImageEventListener(object : SubsamplingScaleImageView.DefaultOnImageEventListener() {
                    override fun onReady() { currentLoaded("${sWidth} × ${sHeight}") }
                    override fun onImageLoadError(e: Exception) {
                        currentError("文件可能已移动、删除，或本地访问权限已失效。")
                    }
                    override fun onTileLoadError(e: Exception) { currentError("本地图像分块解码失败，请重试读取。") }
                })
                setImage(ImageSource.uri(Uri.parse(item.uri!!)).tilingEnabled())
            }
        }, modifier = modifier, onReset = null,
        onRelease = { it.setOnImageEventListener(null); it.recycle() }, update = { }
    )
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun LocalMetadataSheet(item: DownloadItem, onDismiss: () -> Unit) {
    val wall = item.wall
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(24.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Text("本地图片信息", style = MaterialTheme.typography.headlineSmall)
            Text("来自已保存的本地记录，不请求远程详情。", style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant)
            SelectionContainer {
                Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Text("图片 ID：${wall.id}\n分辨率：${wall.resolution}\n类型：${wall.format}")
                    val size = item.total?.takeIf { it > 0 } ?: wall.size.takeIf { it > 0 }
                    Text("文件大小：${size?.let { formatBytes(it) } ?: "未知"}\n分类：${wall.category}\n分级：${wall.purity.uppercase()}\n状态：${localStatus(item.state)}")
                    wall.uploader?.let { Text("上传者：$it") }
                    if (wall.tags.isNotEmpty()) Text("标签：${wall.tags.joinToString("、") { it.second }}")
                    Text("本地位置：${item.uri.orEmpty()}", style = MaterialTheme.typography.bodySmall)
                }
            }
            Spacer(Modifier.height(12.dp))
        }
    }
}
