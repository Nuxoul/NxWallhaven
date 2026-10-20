package io.github.nuxoul.nxwallhaven.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.sizeIn
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Explore
import androidx.compose.material.icons.outlined.Settings
import androidx.compose.material.icons.outlined.FavoriteBorder
import androidx.compose.material.icons.outlined.PhotoLibrary
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.compose.foundation.lazy.staggeredgrid.rememberLazyStaggeredGridState
import androidx.compose.foundation.lazy.staggeredgrid.LazyStaggeredGridState
import kotlinx.coroutines.flow.distinctUntilChanged
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.nuxoul.nxwallhaven.NxViewModel
import java.util.Locale

private val NightColors = darkColorScheme(
    primary = Color(0xFF79B8FF), onPrimary = Color(0xFF082B4E),
    primaryContainer = Color(0xFF203E60), onPrimaryContainer = Color(0xFFD4E7FF),
    secondary = Color(0xFFA7AFBD), onSecondary = Color(0xFF252830),
    background = Color(0xFF141518), onBackground = Color(0xFFEDF0F5),
    surface = Color(0xFF1B1D21), onSurface = Color(0xFFEDF0F5),
    surfaceVariant = Color(0xFF252830), onSurfaceVariant = Color(0xFFA7AFBD),
    surfaceContainer = Color(0xFF1B1D21), surfaceContainerHigh = Color(0xFF252830),
    surfaceContainerHighest = Color(0xFF30343C), outline = Color(0xFF606875),
    error = Color(0xFFFFB4AB)
)
private val DayColors = lightColorScheme(
    primary = Color(0xFF225F9A), onPrimary = Color.White,
    primaryContainer = Color(0xFFD4E7FF), onPrimaryContainer = Color(0xFF082B4E),
    background = Color(0xFFF5F6FA), onBackground = Color(0xFF191C21),
    surface = Color.White, onSurface = Color(0xFF191C21),
    surfaceVariant = Color(0xFFE7EAF0), onSurfaceVariant = Color(0xFF505967),
    surfaceContainer = Color(0xFFEEF0F5), surfaceContainerHigh = Color(0xFFE7EAF0)
)

/** The view model owns navigation and every network/disk operation. */
@Composable
fun NxApp(vm: NxViewModel) {
    val state by vm.ui.collectAsStateWithLifecycle()
    val dark = when (state.prefs.theme) { 1 -> true; 2 -> false; else -> isSystemInDarkTheme() }
    MaterialTheme(colorScheme = if (dark) NightColors else DayColors) {
        val snackbar = remember { SnackbarHostState() }
        val browsePosition=remember(vm) { vm.consumeGridRestoration(false) }
        val profilePosition=remember(vm) { vm.consumeGridRestoration(true) }
        val browseGrid = rememberLazyStaggeredGridState()
        val profileGrid = rememberLazyStaggeredGridState()
        PersistGrid(vm,browseGrid,false,browsePosition,state.walls.size,state.generation)
        PersistGrid(vm,profileGrid,true,profilePosition,state.profileWalls.size,state.profileGeneration)
        val accountGrid = rememberLazyStaggeredGridState()
        var underlyingRoute by rememberSaveable { mutableStateOf(vm.detailBaseRoute()) }
        // A detail is an overlay, not a replacement: its source grid stays composed.
        SideEffect { if (state.route != "detail") underlyingRoute = state.route }
        val baseRoute = if (state.route == "detail") underlyingRoute else state.route
        BackHandler(enabled = state.route != "browse") { vm.back() }
        LaunchedEffect(state.message) {
            state.message?.let { snackbar.showSnackbar(it); vm.clearMessage() }
        }
        Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
            Box(Modifier.fillMaxSize()) {
                when (baseRoute) {
                    "settings" -> SettingsScreen(vm)
                    "profile" -> ProfileScreen(vm, profileGrid, active = state.route == "profile")
                    "collections" -> AccountScreen(vm, accountGrid, active = state.route == "collections")
                    "local" -> LocalScreen(vm)
                    else -> DiscoverScreen(vm, browseGrid, active = state.route == "browse")
                }
                if (state.route == "detail") DetailScreen(vm)
                AccountOverlays(vm)
                SnackbarHost(snackbar, Modifier.align(Alignment.BottomCenter).padding(bottom = 88.dp))
            }
        }
    }
}

/** Restore only loaded data. Never fetch every intervening page to chase a stale scroll index. */
@Composable
private fun PersistGrid(vm:NxViewModel,grid:LazyStaggeredGridState,profile:Boolean,position:Pair<Int,Int>?,size:Int,generation:Long) {
    var restored by remember { mutableStateOf(position==null) }
    LaunchedEffect(size>0) {
        if(size>0 && !restored) {
            // Gallery applies its new-generation reset first.
            withFrameNanos { };withFrameNanos { }
            if(position!=null && position.first<size)grid.scrollToItem(position.first,position.second)
            restored=true
        }
    }
    LaunchedEffect(grid,restored,generation,size>0) {
        if(restored && size>0)snapshotFlow { grid.firstVisibleItemIndex to grid.firstVisibleItemScrollOffset }
            .distinctUntilChanged().collect { vm.saveGridPosition(profile,it.first,it.second) }
    }
}

@Composable
internal fun AppNavigation(settings: Boolean, vm: NxViewModel) {
    val state by vm.ui.collectAsStateWithLifecycle()
    NavigationBar(containerColor = MaterialTheme.colorScheme.surface) {
        NavigationBarItem(
            selected = state.route == "browse", onClick = { vm.browse() },
            icon = { Icon(Icons.Outlined.Explore, contentDescription = "浏览壁纸") },
            label = { Text("浏览") }, modifier = Modifier.testTag("nav-discover")
        )
        NavigationBarItem(
            selected = state.route == "collections", onClick = { vm.collections() },
            icon = { Icon(Icons.Outlined.FavoriteBorder, contentDescription = "我的收藏") },
            label = { Text("收藏") }, modifier = Modifier.testTag("nav-collections")
        )
        NavigationBarItem(
            selected = state.route == "local", onClick = { vm.localPage() },
            icon = { Icon(Icons.Outlined.PhotoLibrary, contentDescription = "下载与本地图库") },
            label = { Text("本地") }, modifier = Modifier.testTag("nav-local")
        )
        NavigationBarItem(
            selected = state.route == "settings", onClick = { vm.settings() },
            icon = { Icon(Icons.Outlined.Settings, contentDescription = "设置") },
            label = { Text("设置") }, modifier = Modifier.testTag("nav-settings")
        )
    }
}

@Composable
internal fun NamedIcon(
    icon: ImageVector, label: String, onClick: () -> Unit,
    modifier: Modifier = Modifier, enabled: Boolean = true
) {
    IconButton(onClick = onClick, enabled = enabled, modifier = modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp)) {
        Icon(icon, contentDescription = label)
    }
}

internal fun formatBytes(bytes: Long): String {
    val value = bytes.coerceAtLeast(0)
    return when {
        value >= 1_073_741_824 -> String.format(Locale.getDefault(), "%.2f GiB", value / 1_073_741_824.0)
        value >= 1_048_576 -> String.format(Locale.getDefault(), "%.1f MiB", value / 1_048_576.0)
        value >= 1024 -> String.format(Locale.getDefault(), "%.1f KiB", value / 1024.0)
        else -> "$value B"
    }
}
