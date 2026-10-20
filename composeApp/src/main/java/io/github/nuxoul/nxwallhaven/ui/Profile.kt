package io.github.nuxoul.nxwallhaven.ui

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.staggeredgrid.LazyStaggeredGridState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.nuxoul.nxwallhaven.NxViewModel

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun ProfileScreen(vm: NxViewModel, gridState: LazyStaggeredGridState, active: Boolean) {
    val state by vm.ui.collectAsStateWithLifecycle()
    var collectionsOpen by remember { mutableStateOf(false) }
    LaunchedEffect(state.profileTab, state.collections, state.selectedCollection, active) {
        if (active && state.profileTab == 2 && state.selectedCollection == null) {
            state.collections.firstOrNull()?.let { vm.openCollection(it) }
        }
    }
    Scaffold(
        modifier = Modifier.fillMaxSize().testTag("profile"),
        containerColor = MaterialTheme.colorScheme.background,
        topBar = {
            Column {
                TopAppBar(
                    title = {
                        Column {
                            Text(state.profileUsername, style = MaterialTheme.typography.titleLarge)
                            Text("Wallhaven 作者", style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    },
                    navigationIcon = { NamedIcon(Icons.AutoMirrored.Outlined.ArrowBack, "返回", { vm.back() }) }
                )
                TabRow(selectedTabIndex = state.profileTab.coerceIn(0, 2)) {
                    listOf("资料", "上传", "收藏集").forEachIndexed { index, label ->
                        Tab(selected = state.profileTab == index, onClick = { vm.profileTab(index) }, text = { Text(label) },
                            modifier = Modifier.heightIn(min = 48.dp).testTag("profile-tab-$index"))
                    }
                }
                if (state.profileTab != 0) {
                    Column(Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) {
                        Text("作者内容分级 · 不影响浏览页", style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant)
                        PurityChips(state.profilePurity, state.prefs.hasKey && state.prefs.allowNsfw) { vm.profileFilters(it) }
                        if (state.profileTab == 2) {
                            Box {
                                OutlinedButton(onClick = { collectionsOpen = true }, enabled = state.collections.isNotEmpty(),
                                    modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag("collections-chooser")) {
                                    Icon(Icons.Outlined.Collections, contentDescription = null)
                                    Spacer(Modifier.width(8.dp))
                                    Text(state.selectedCollection?.let { "${it.label} · ${it.count}" } ?: "选择收藏集",
                                        modifier = Modifier.weight(1f))
                                    Icon(Icons.Outlined.ArrowDropDown, contentDescription = "选择收藏集")
                                }
                                DropdownMenu(expanded = collectionsOpen, onDismissRequest = { collectionsOpen = false }) {
                                    state.collections.forEach { collection ->
                                        DropdownMenuItem(text = { Text("${collection.label} · ${collection.count}") },
                                            onClick = { collectionsOpen = false; vm.openCollection(collection) })
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    ) { padding ->
        if (state.profileTab == 0) {
            LazyColumn(
                modifier = Modifier.fillMaxSize().padding(padding), contentPadding = PaddingValues(24.dp),
                verticalArrangement = Arrangement.spacedBy(20.dp)
            ) {
                val author = state.profile
                if (author != null) {
                    item {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                            if (!author.avatar.isNullOrBlank()) DiskImage(vm, author.avatar, "${author.username} 的头像",
                                Modifier.size(80.dp).clip(CircleShape), ContentScale.Crop)
                            else Icon(Icons.Outlined.AccountCircle, "作者头像", Modifier.size(80.dp), tint = MaterialTheme.colorScheme.primary)
                            Column {
                                Text(author.username, style = MaterialTheme.typography.headlineSmall)
                                Text(author.group, color = MaterialTheme.colorScheme.primary, style = MaterialTheme.typography.labelLarge)
                            }
                        }
                    }
                    item {
                        Text("简介", style = MaterialTheme.typography.titleMedium)
                        Spacer(Modifier.height(8.dp))
                        SelectionContainer { Text(author.biography, style = MaterialTheme.typography.bodyLarge) }
                    }
                    author.fields.forEach { (label, value) ->
                        item {
                            Surface(shape = MaterialTheme.shapes.medium, color = MaterialTheme.colorScheme.surface) {
                                Column(Modifier.fillMaxWidth().padding(16.dp)) {
                                    Text(label, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.labelLarge)
                                    Spacer(Modifier.height(4.dp))
                                    SelectionContainer { Text(value, style = MaterialTheme.typography.bodyLarge) }
                                }
                            }
                        }
                    }
                    item {
                        FilledTonalButton(onClick = { vm.profileTab(1) }, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) {
                            Icon(Icons.Outlined.PhotoLibrary, contentDescription = null)
                            Spacer(Modifier.width(8.dp)); Text("查看上传")
                        }
                    }
                } else {
                    item {
                        Column(Modifier.fillMaxWidth().padding(top = 32.dp), horizontalAlignment = Alignment.CenterHorizontally,
                            verticalArrangement = Arrangement.spacedBy(16.dp)) {
                            if (state.profileError != null) {
                                Text(state.profileError ?: "无法加载作者资料", color = MaterialTheme.colorScheme.error)
                                Button(onClick = { vm.openProfile(state.profileUsername) }) { Text("重试") }
                            } else {
                                CircularProgressIndicator()
                                Text("正在加载作者资料…")
                            }
                        }
                    }
                }
            }
        } else if (state.profileTab == 2 && state.collections.isEmpty() && !state.profileLoading && state.profileError == null) {
            Box(Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.Center) {
                Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Icon(Icons.Outlined.Collections, contentDescription = null, modifier = Modifier.size(40.dp))
                    Text("没有公开收藏集", style = MaterialTheme.typography.titleMedium)
                }
            }
        } else {
            Gallery(
                vm = vm, walls = state.profileWalls, loading = state.profileLoading, exhausted = state.profileExhausted,
                error = state.profileError, generation = state.profileGeneration, equalGrid = state.prefs.grid,
                gridState = gridState, active = active, onMore = { vm.loadMore(profile = true) },
                onRefresh = { vm.profileFilters(state.profilePurity) },
                modifier = Modifier.fillMaxSize().padding(padding).testTag("profile-gallery")
            )
        }
    }
}
