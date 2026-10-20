package io.github.nuxoul.nxwallhaven.ui

import android.content.res.Configuration
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.staggeredgrid.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.*
import androidx.compose.material3.*
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import coil3.compose.AsyncImage
import io.github.nuxoul.nxwallhaven.NxViewModel
import io.github.nuxoul.nxwallhaven.shared.Wall
import io.github.nuxoul.nxwallhaven.shared.GalleryPrefetch
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.distinctUntilChanged
import java.io.File
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.filter
import kotlinx.coroutines.flow.take
import kotlinx.coroutines.withContext

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun DiscoverScreen(vm: NxViewModel, gridState: LazyStaggeredGridState, active: Boolean) {
    val state by vm.ui.collectAsStateWithLifecycle()
    var query by rememberSaveable(state.filters.query) { mutableStateOf(state.filters.query) }
    var filtersOpen by rememberSaveable { mutableStateOf(false) }
    val keyboard = LocalSoftwareKeyboardController.current
    val submit = {
        keyboard?.hide()
        vm.search(state.filters.copy(query = query.trim()))
    }
    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        bottomBar = { AppNavigation(false, vm) },
        topBar = {
            Column {
                TopAppBar(
                    title = {
                        Column {
                            Text("NxWallhaven", style = MaterialTheme.typography.titleLarge)
                            Text("按原图比例浏览壁纸", style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    },
                    actions = {
                        BatchDownloadButton(vm,state.walls)
                        NamedIcon(Icons.Outlined.Tune, "搜索筛选", { filtersOpen = true }, Modifier.testTag("filters"))
                    }
                )
                OutlinedTextField(
                    value = query, onValueChange = { query = it }, singleLine = true,
                    placeholder = { Text("搜索壁纸或 @作者") },
                    leadingIcon = { NamedIcon(Icons.Outlined.Search, "搜索", submit, Modifier.testTag("search")) },
                    trailingIcon = if (query.isNotEmpty()) ({
                        NamedIcon(Icons.Outlined.Close, "清空搜索", { query = "" })
                    }) else null,
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                    keyboardActions = KeyboardActions(onSearch = { submit() }),
                    shape = RoundedCornerShape(16.dp),
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp).padding(bottom = 12.dp).testTag("query")
                )
            }
        }
    ) { padding ->
        Gallery(
            vm = vm, walls = state.walls, loading = state.loading, exhausted = state.exhausted,
            error = state.error, generation = state.generation, equalGrid = state.prefs.grid,
            gridState = gridState, active = active, onMore = { vm.loadMore() }, onRefresh = { vm.refresh() },
            modifier = Modifier.fillMaxSize().padding(padding).testTag("gallery")
        )
    }
    if (filtersOpen) FilterSheet(
        initial = state.filters.copy(query = query), nsfwAllowed = state.prefs.hasKey && state.prefs.allowNsfw,
        onDismiss = { filtersOpen = false }, onApply = { filtersOpen = false; vm.search(it) }
    )
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun Gallery(
    vm: NxViewModel, walls: List<Wall>, loading: Boolean, exhausted: Boolean, error: String?,
    generation: Long, equalGrid: Boolean, gridState: LazyStaggeredGridState, active: Boolean,
    onMore: () -> Unit, onRefresh: () -> Unit, modifier: Modifier = Modifier
) {
    val columns = if (LocalConfiguration.current.orientation == Configuration.ORIENTATION_LANDSCAPE) 3 else 2
    var positionedGeneration by rememberSaveable {
        mutableLongStateOf(if (walls.isNotEmpty()) generation else Long.MIN_VALUE)
    }
    LaunchedEffect(generation, walls.isEmpty()) {
        if (walls.isNotEmpty() && positionedGeneration != generation) {
            gridState.scrollToItem(0)
            positionedGeneration = generation
        }
    }
    LaunchedEffect(gridState, walls.size, loading, exhausted, error, active) {
        if (active && walls.isNotEmpty() && !loading && !exhausted && error == null) {
            snapshotFlow {
                val visible=gridState.layoutInfo.visibleItemsInfo.filter{it.index<walls.size}
                GalleryPrefetch.shouldLoad(walls.size,visible.maxOfOrNull{it.index}?:-1,visible.size)
            }.filter { it }.take(1).collect { onMore() }
        }
    }
    LaunchedEffect(gridState,walls,generation,active) {
        if(active && walls.isNotEmpty())snapshotFlow {
            val visible=gridState.layoutInfo.visibleItemsInfo.filter{it.index<walls.size}
            val first=visible.minOfOrNull{it.index}?:0
            first to GalleryPrefetch.thumbnailEnd(walls.size,visible.maxOfOrNull{it.index}?:-1,visible.size)
        }.distinctUntilChanged().collectLatest { (first,end) ->
            if(end>first)vm.prefetchThumbnails(walls.subList(first,end).take(72))
        }
    }
    PullToRefreshBox(isRefreshing = loading && walls.isEmpty(), onRefresh = onRefresh, modifier = modifier) {
        LazyVerticalStaggeredGrid(
            columns = StaggeredGridCells.Fixed(columns), state = gridState,
            horizontalArrangement = Arrangement.spacedBy(10.dp), verticalItemSpacing = 10.dp,
            contentPadding = PaddingValues(start = 12.dp, end = 12.dp, top = 6.dp, bottom = 24.dp),
            modifier = Modifier.fillMaxSize().testTag("wallpaper-grid")
        ) {
            items(walls, key = { it.id }) { wall ->
                WallpaperCard(wall, vm, equalGrid, onClick = { vm.openDetail(wall) })
            }
            item(key = "gallery-status", span = StaggeredGridItemSpan.FullLine) {
                Column(
                    Modifier.fillMaxWidth().padding(vertical = 24.dp),
                    horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)
                ) {
                    when {
                        loading -> {
                            CircularProgressIndicator(Modifier.size(28.dp), strokeWidth = 2.dp)
                            Text(if (walls.isEmpty()) "正在查找壁纸…" else "正在加载更多…",
                                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                        error != null -> {
                            Text(error, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(horizontal = 12.dp))
                            OutlinedButton(onClick = if (walls.isEmpty()) onRefresh else onMore) { Text("重试") }
                        }
                        walls.isEmpty() -> {
                            Icon(Icons.Outlined.ImageSearch, contentDescription = null, modifier = Modifier.size(40.dp))
                            Text("未找到壁纸", style = MaterialTheme.typography.titleMedium)
                            Text("请更换搜索词或放宽筛选条件。", color = MaterialTheme.colorScheme.onSurfaceVariant)
                            OutlinedButton(onClick = onRefresh) { Text("刷新") }
                        }
                        exhausted -> Text("已加载全部 ${walls.size} 张壁纸",
                            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            }
        }
    }
}

@Composable
private fun WallpaperCard(wall: Wall, vm: NxViewModel, equalGrid: Boolean, onClick: () -> Unit) {
    Card(
        onClick = onClick, shape = RoundedCornerShape(12.dp),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
        modifier = Modifier.fillMaxWidth().testTag("card-${wall.id}")
            .semantics { contentDescription = "壁纸 ${wall.id}，${wall.resolution}，${wall.purity}" }
    ) {
        Box(Modifier.fillMaxWidth().aspectRatio(if (equalGrid) 1f else wall.aspect.coerceIn(0.2f, 5f))) {
            DiskImage(
                vm, wall.thumbnail, "壁纸 ${wall.id}", Modifier.fillMaxSize(),
                scale = ContentScale.Fit
            )
            Surface(
                color = MaterialTheme.colorScheme.surface.copy(alpha = 0.88f),
                shape = RoundedCornerShape(topStart = 8.dp),
                modifier = Modifier.align(Alignment.BottomEnd)
            ) {
                Text(wall.resolution, Modifier.padding(horizontal = 8.dp, vertical = 4.dp),
                    style = MaterialTheme.typography.labelSmall)
            }
        }
    }
}

private data class DiskImageState(val file: File? = null, val error: Boolean = false)

/** Coil sees only a local file. Rust-backed acquisition remains in the view model. */
@Composable
internal fun DiskImage(
    vm: NxViewModel, url: String?, description: String?, modifier: Modifier = Modifier,
    scale: ContentScale = ContentScale.Fit
) {
    var attempt by remember(url) { mutableIntStateOf(0) }
    val image by produceState(DiskImageState(), url, attempt) {
        value = DiskImageState()
        if (url.isNullOrBlank()) { value = DiskImageState(error = true); return@produceState }
        try {
            val file = withContext(Dispatchers.IO) { vm.image(url) }
            value = DiskImageState(file = file)
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Exception) {
            value = DiskImageState(error = true)
        }
    }
    var decodeError by remember(image.file) { mutableStateOf(false) }
    var decoded by remember(image.file) { mutableStateOf(false) }
    Box(modifier.background(MaterialTheme.colorScheme.surfaceVariant), contentAlignment = Alignment.Center) {
        if (image.file != null && !decodeError) {
            AsyncImage(model = image.file, contentDescription = description, contentScale = scale,
                modifier = Modifier.fillMaxSize().then(if(decoded) Modifier.testTag("image-loaded") else Modifier),
                onSuccess = { decoded = true }, onError = { decodeError = true })
        } else if (image.error || decodeError) {
            NamedIcon(Icons.Outlined.Refresh, "重试加载图片", { decodeError = false; attempt++ })
        } else {
            CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp)
        }
    }
}
