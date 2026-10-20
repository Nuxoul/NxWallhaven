package io.github.nuxoul.nxwallhaven.ui

import android.net.Uri
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.davemorrissey.labs.subscaleview.ImageSource
import com.davemorrissey.labs.subscaleview.SubsamplingScaleImageView
import io.github.nuxoul.nxwallhaven.NxViewModel
import io.github.nuxoul.nxwallhaven.shared.Wall
import java.io.File

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun DetailScreen(vm: NxViewModel) {
    val state by vm.ui.collectAsStateWithLifecycle()
    val wall = state.detail
    var metadata by rememberSaveable(wall?.id) { mutableStateOf(false) }
    var more by remember { mutableStateOf(false) }
    var wallpaperTarget by rememberSaveable { mutableStateOf<Boolean?>(null) }
    var loadedDimensions by remember(state.detailFile) { mutableStateOf<String?>(null) }
    var tileError by remember(state.detailFile) { mutableStateOf<String?>(null) }
    var retry by remember { mutableIntStateOf(0) }
    val available = wall?.original != null && tileError == null && !state.actionBusy
    Surface(Modifier.fillMaxSize().testTag("detail"), color = MaterialTheme.colorScheme.background) {
        Scaffold(
            containerColor = MaterialTheme.colorScheme.background,
            topBar = {
                TopAppBar(
                    title = {
                        Column {
                            Text(wall?.let { "壁纸 ${it.id}" } ?: "壁纸", style = MaterialTheme.typography.titleMedium)
                            Text(wall?.resolution ?: "原图", style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    },
                    navigationIcon = { NamedIcon(Icons.AutoMirrored.Outlined.ArrowBack, "返回", { vm.back() }, Modifier.testTag("detail-back")) },
                    actions = {
                        NamedIcon(Icons.Outlined.FavoriteBorder, "管理此图收藏", { wall?.let { vm.account.favorite(it.id) } },
                            Modifier.testTag("favorite-wallpaper"), enabled = wall != null)
                        NamedIcon(Icons.Outlined.Info, "壁纸信息", { metadata = true },
                            Modifier.testTag("detail-info"), enabled = wall != null)
                    }
                )
            },
            bottomBar = {
                Surface(color = MaterialTheme.colorScheme.surface) {
                    Column(Modifier.navigationBarsPadding().padding(horizontal = 16.dp, vertical = 8.dp)) {
                        if (state.actionBusy) LinearProgressIndicator(Modifier.fillMaxWidth().padding(bottom = 8.dp))
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Button(onClick = { vm.saveOriginal() }, enabled = available,
                                modifier = Modifier.weight(1f).heightIn(min = 48.dp).testTag("save-original")) {
                                Icon(Icons.Outlined.Download, contentDescription = null)
                                Spacer(Modifier.width(8.dp))
                                Text("保存原图")
                            }
                            NamedIcon(Icons.Outlined.Share, "分享原图文件", { vm.shareOriginal() },
                                Modifier.testTag("share-original"), enabled = available)
                            Box {
                                NamedIcon(Icons.Outlined.MoreVert, "更多原图操作", { more = true }, enabled = available)
                                DropdownMenu(expanded = more, onDismissRequest = { more = false }) {
                                    DropdownMenuItem(text = { Text("设为主屏幕壁纸") },
                                        leadingIcon = { Icon(Icons.Outlined.Wallpaper, contentDescription = null) },
                                        onClick = { more = false; wallpaperTarget = false }, modifier = Modifier.testTag("wallpaper-home"))
                                    DropdownMenuItem(text = { Text("设为锁屏壁纸") },
                                        leadingIcon = { Icon(Icons.Outlined.Lock, contentDescription = null) },
                                        onClick = { more = false; wallpaperTarget = true }, modifier = Modifier.testTag("wallpaper-lock"))
                                    DropdownMenuItem(text = { Text("导出原图到…") },
                                        leadingIcon = { Icon(Icons.Outlined.FolderOpen, contentDescription = null) },
                                        onClick = { more = false; vm.exportOriginal() }, modifier = Modifier.testTag("export-original"))
                                }
                            }
                        }
                    }
                }
            }
        ) { padding ->
            Column(Modifier.fillMaxSize().padding(padding)) {
                Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
                    val original = state.detailFile
                    if (original != null && state.detailError == null) {
                        key(original.absolutePath, retry) {
                            OriginalImage(original,
                                onLoaded = { loadedDimensions = it }, onError = { tileError = it },
                                modifier = Modifier.fillMaxSize().testTag("original-image"))
                        }
                    }
                    val error = state.detailError ?: tileError
                    if (error != null) {
                        Surface(color = MaterialTheme.colorScheme.surface.copy(alpha = 0.96f)) {
                            Column(Modifier.padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally,
                                verticalArrangement = Arrangement.spacedBy(16.dp)) {
                                Icon(Icons.Outlined.BrokenImage, contentDescription = null, modifier = Modifier.size(40.dp))
                                Text("无法加载原图", style = MaterialTheme.typography.titleMedium)
                                Text(error, color = MaterialTheme.colorScheme.error)
                                Button(onClick = { tileError = null; loadedDimensions = null; retry++; vm.retryDetail() },
                                    modifier = Modifier.heightIn(min = 48.dp).testTag("retry-original")) { Text("重试原图") }
                            }
                        }
                    } else if (original == null || loadedDimensions == null) {
                        Surface(shape = MaterialTheme.shapes.large, color = MaterialTheme.colorScheme.surface.copy(alpha = 0.94f)) {
                            Column(Modifier.padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally,
                                verticalArrangement = Arrangement.spacedBy(12.dp)) {
                                CircularProgressIndicator(Modifier.size(30.dp))
                                Text(state.detailStage.ifBlank { if (original == null) "正在获取原图…" else "正在准备原图本地分块…" },
                                    modifier = Modifier.testTag("detail-stage"))
                                Text("显示完整原图，按需加载本地分块", style = MaterialTheme.typography.bodySmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant)
                            }
                        }
                    }
                }
                Text(
                    loadedDimensions?.let { "原图已显示 · $it · 双指缩放，最高 8×" }
                        ?: state.detailStage.ifBlank { "等待原图" },
                    style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp).testTag("original-dimensions")
                )
            }
        }
    }
    if (metadata && wall != null) MetadataSheet(wall, vm, onDismiss = { metadata = false })
    wallpaperTarget?.let { lock ->
        AlertDialog(
            onDismissRequest = { wallpaperTarget = null },
            title = { Text(if (lock) "设为锁屏壁纸？" else "设为主屏幕壁纸？") },
            text = { Text(if (lock) "将当前原图设为锁屏壁纸。" else "将当前原图设为主屏幕壁纸。") },
            confirmButton = {
                TextButton(onClick = { wallpaperTarget = null; vm.wallpaper(lock) }) { Text("确认设置") }
            },
            dismissButton = { TextButton(onClick = { wallpaperTarget = null }) { Text("取消") } }
        )
    }
}

/** Region decoding keeps large originals off the Java bitmap heap. */
@Composable
private fun OriginalImage(file: File, onLoaded: (String) -> Unit, onError: (String) -> Unit, modifier: Modifier) {
    val currentLoaded by rememberUpdatedState(onLoaded)
    val currentError by rememberUpdatedState(onError)
    AndroidView(
        factory = { context ->
            SubsamplingScaleImageView(context).apply {
                contentDescription = "完整原图。双指缩放或双击放大。"
                setOrientation(SubsamplingScaleImageView.ORIENTATION_USE_EXIF)
                setMinimumScaleType(SubsamplingScaleImageView.SCALE_TYPE_CENTER_INSIDE)
                setMaxScale(8f)
                setMaxTileSize(2048)
                setMinimumTileDpi(160)
                setPanEnabled(true)
                setZoomEnabled(true)
                setOnImageEventListener(object : SubsamplingScaleImageView.DefaultOnImageEventListener() {
                    override fun onReady() { currentLoaded("${sWidth} × ${sHeight}") }
                    override fun onImageLoadError(e: Exception) { currentError(e.message ?: "原图解码失败") }
                    override fun onTileLoadError(e: Exception) { currentError(e.message ?: "图像分块解码失败") }
                })
                setImage(ImageSource.uri(Uri.fromFile(file)).tilingEnabled())
            }
        },
        modifier = modifier,
        onReset = null,
        onRelease = { it.recycle() },
        update = { }
    )
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
private fun MetadataSheet(wall: Wall, vm: NxViewModel, onDismiss: () -> Unit) {
    val state by vm.ui.collectAsStateWithLifecycle()
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(24.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Text("关于此壁纸", style = MaterialTheme.typography.headlineSmall)
            SelectionContainer {
                Text("${wall.resolution}  ·  ${formatBytes(wall.size)}\n${wall.category}  ·  ${wall.purity.uppercase()}  ·  ${wall.format}")
            }
            wall.uploader?.let { uploader ->
                OutlinedButton(onClick = { onDismiss(); vm.openProfile(uploader) },
                    modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag("open-profile")) {
                    Icon(Icons.Outlined.Person, contentDescription = null)
                    Spacer(Modifier.width(8.dp)); Text("上传者：$uploader")
                }
            }
            if (wall.views != null || wall.favorites != null) {
                Text(listOfNotNull(wall.views?.let { "$it 次浏览" }, wall.favorites?.let { "$it 次收藏" }).joinToString(" · "),
                    color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodyMedium)
            }
            if (wall.tags.isNotEmpty()) {
                Text("标签", style = MaterialTheme.typography.titleMedium)
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    wall.tags.forEach { (id, name) ->
                        SuggestionChip(onClick = { onDismiss(); vm.search(state.filters.copy(query = "id:$id")) },
                            label = { Text(name) }, modifier = Modifier.heightIn(min = 48.dp))
                    }
                }
            }
            if (wall.colors.isNotEmpty()) {
                Text("配色 · 点击探索", style = MaterialTheme.typography.titleMedium)
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    wall.colors.forEach { hex ->
                        val color = runCatching { Color(android.graphics.Color.parseColor(if (hex.startsWith("#")) hex else "#$hex")) }.getOrNull()
                        if (color != null) {
                            FilledTonalIconButton(
                                onClick = { onDismiss(); vm.search(state.filters.copy(colors = hex.removePrefix("#"))) },
                                modifier = Modifier.size(48.dp).semantics { contentDescription = "查找颜色为 $hex 的壁纸" }
                            ) { Box(Modifier.size(32.dp).clip(CircleShape).background(color)) }
                        }
                    }
                }
            }
            Spacer(Modifier.height(12.dp))
        }
    }
}
