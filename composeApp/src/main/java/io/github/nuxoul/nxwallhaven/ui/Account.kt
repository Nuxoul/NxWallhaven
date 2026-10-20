package io.github.nuxoul.nxwallhaven.ui

import android.widget.FrameLayout
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.staggeredgrid.LazyStaggeredGridState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.Add
import androidx.compose.material.icons.outlined.MoreVert
import androidx.compose.material.icons.outlined.Refresh
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.BottomSheetDefaults
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SheetValue
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.github.nuxoul.nxwallhaven.NxViewModel
import io.github.nuxoul.nxwallhaven.account.AccountController
import io.github.nuxoul.nxwallhaven.account.AccountState
import io.github.nuxoul.nxwallhaven.shared.CollectionInfo

private data class CollectionAction(val kind: String, val collection: CollectionInfo? = null)

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun AccountScreen(vm: NxViewModel, gridState: LazyStaggeredGridState, active: Boolean, account: AccountController = vm.account) {
    val state by account.ui.collectAsStateWithLifecycle()
    val app by vm.ui.collectAsStateWithLifecycle()
    val locked = state.busy || state.manageBusy || state.favoriteBusy
    var action by remember(state.username) { mutableStateOf<CollectionAction?>(null) }
    var logoutOpen by remember { mutableStateOf(false) }
    val selected = state.selected
    BackHandler(enabled = active && selected != null && !state.loginOpen && !state.favoriteOpen) {
        if (!locked) account.closeCollection()
    }
    Scaffold(
        modifier = Modifier.fillMaxSize().testTag("account"),
        containerColor = MaterialTheme.colorScheme.background,
        bottomBar = { AppNavigation(false, vm) },
        topBar = {
            Column {
                TopAppBar(
                    title = {
                        Column {
                            Text("我的收藏")
                            if (selected != null) Text(selected.label, style = MaterialTheme.typography.bodySmall)
                        }
                    },
                    navigationIcon = {
                        if (selected != null) NamedIcon(
                            Icons.AutoMirrored.Outlined.ArrowBack, "返回收藏夹", { account.closeCollection() },
                            enabled = !locked
                        )
                    },
                    actions = { if(selected!=null)BatchDownloadButton(vm,state.walls) }
                )
                if (selected != null) {
                    Column(Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) {
                        Text("收藏内容分级 · 不影响浏览页", style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant)
                        AccountPurityChips(state.purity, app.prefs.hasKey && app.prefs.allowNsfw, !locked) {
                            account.filters(it)
                        }
                    }
                }
            }
        }
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding)) {
            state.manageMessage?.let { message ->
                Surface(color = MaterialTheme.colorScheme.secondaryContainer) {
                    Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 8.dp),
                        verticalAlignment = Alignment.CenterVertically) {
                        Text(message, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium)
                        TextButton(onClick = { account.clearManageMessage() }, enabled = !locked,
                            modifier = Modifier.heightIn(min = 48.dp).testTag("clear-management-message")) { Text("知道了") }
                    }
                }
            }
            if (selected == null) {
                LazyColumn(
                    modifier = Modifier.fillMaxSize(), contentPadding = PaddingValues(16.dp),
                    verticalArrangement = Arrangement.spacedBy(12.dp)
                ) {
                    item(key = "session") {
                        Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant)) {
                            Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                Text(state.username?.let { "网站账号：$it" } ?: "尚未核对网站登录",
                                    style = MaterialTheme.typography.titleMedium)
                                Text(state.status, color = MaterialTheme.colorScheme.onSurfaceVariant)
                                if(state.busy) {
                                    LinearProgressIndicator(Modifier.fillMaxWidth())
                                    Text("核对有超时限制；若网站要求安全验证，请稍后重新打开网页完成。",style=MaterialTheme.typography.bodySmall)
                                }
                                Text("网页登录与 API Key 相互独立；收藏操作会修改 Wallhaven 网站数据。",
                                    style = MaterialTheme.typography.bodySmall)
                                Button(onClick = { account.openLogin() }, enabled = !locked,
                                    modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag("account-login")) {
                                    Text(if (state.username == null) "打开网站登录" else "打开网站登录 / 验证")
                                }
                                OutlinedButton(onClick = { account.verify() }, enabled = !locked,
                                    modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag("account-verify")) {
                                    Text("核对登录状态")
                                }
                                OutlinedButton(onClick = { account.refreshCollections() }, enabled = !locked,
                                    modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag("refresh-collections")) {
                                    Text("刷新收藏夹")
                                }
                                TextButton(onClick = { logoutOpen = true }, enabled = !locked,
                                    modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag("account-logout")) {
                                    Text("退出网站登录", color = MaterialTheme.colorScheme.error)
                                }
                            }
                        }
                    }
                    item(key = "create") {
                        FilledTonalButton(
                            onClick = { account.clearManageMessage(); action = CollectionAction("create") },
                            enabled = !locked && state.username != null,
                            modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag("new-collection")
                        ) {
                            Icon(Icons.Outlined.Add, contentDescription = null)
                            Spacer(Modifier.width(8.dp)); Text("新建收藏夹")
                        }
                    }
                    items(state.collections, key = { it.id }) { collection ->
                        AccountCollectionCard(collection, !locked,
                            onOpen = { account.openCollection(collection) },
                            onAction = { kind ->
                                account.clearManageMessage(); action = CollectionAction(kind, collection)
                            })
                    }
                    item(key = "account-status") {
                        Column(Modifier.fillMaxWidth().padding(vertical = 16.dp),
                            horizontalAlignment = Alignment.CenterHorizontally,
                            verticalArrangement = Arrangement.spacedBy(8.dp)) {
                            if (state.busy || state.manageBusy) {
                                CircularProgressIndicator(Modifier.size(28.dp))
                                Text(if (state.manageBusy) "正在操作并核对网站结果…" else "正在读取网站会话…")
                            }
                            state.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                            if (!state.busy && state.collections.isEmpty()) {
                                Text(if (state.username == null) "请登录并核对后刷新收藏夹" else "暂无收藏夹，请刷新或新建",
                                    color = MaterialTheme.colorScheme.onSurfaceVariant)
                            }
                        }
                    }
                }
            } else {
                Gallery(
                    vm = vm, walls = state.walls, loading = state.loading, exhausted = state.exhausted,
                    error = state.error, generation = state.generation, equalGrid = app.prefs.grid,
                    gridState = gridState, active = active && !locked,
                    onMore = { if (!locked) account.loadMore() }, onRefresh = { if (!locked) account.refreshPage() },
                    modifier = Modifier.fillMaxSize().testTag("account-gallery")
                )
            }
        }
    }
    action?.let { request ->
        CollectionManagementDialog(request, locked,
            onDismiss = { action = null },
            onConfirm = { label, public ->
                account.manage(request.kind, request.collection?.id, label, public)
                action = null
            })
    }
    if (logoutOpen) AlertDialog(
        onDismissRequest = { if (!locked) logoutOpen = false },
        title = { Text("退出网站登录？") },
        text = { Text("将清除本应用的网站登录会话。API Key、图片缓存和已下载文件保留。") },
        confirmButton = {
            TextButton(onClick = { logoutOpen = false; account.logout() }, enabled = !locked,
                modifier = Modifier.heightIn(min = 48.dp).testTag("confirm-logout")) { Text("确认退出") }
        },
        dismissButton = {
            TextButton(onClick = { logoutOpen = false }, enabled = !locked,
                modifier = Modifier.heightIn(min = 48.dp)) { Text("取消") }
        }
    )
}

@Composable
private fun AccountPurityChips(value: String, nsfwAllowed: Boolean, enabled: Boolean, onChange: (String) -> Unit) {
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        listOf("SFW", "Sketchy", "NSFW").forEachIndexed { index, label ->
            val selected = value.getOrNull(index) == '1'
            FilterChip(selected = selected, enabled = enabled && (index != 2 || nsfwAllowed),
                onClick = {
                    val bits = value.padEnd(3, '0').take(3).toCharArray()
                    bits[index] = if (selected) '0' else '1'
                    val changed = String(bits)
                    if (changed != "000") onChange(changed)
                }, label = { Text(label) }, modifier = Modifier.heightIn(min = 48.dp).testTag("account-purity-$index"))
        }
    }
}

@Composable
private fun AccountCollectionCard(collection: CollectionInfo, enabled: Boolean, onOpen: () -> Unit, onAction: (String) -> Unit) {
    var expanded by remember(collection.id) { mutableStateOf(false) }
    Card(onClick = onOpen, enabled = enabled,
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
        modifier = Modifier.fillMaxWidth().heightIn(min = 80.dp).testTag("collection-${collection.id}")) {
        Row(Modifier.fillMaxWidth().padding(start = 16.dp, top = 8.dp, bottom = 8.dp, end = 4.dp),
            verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(collection.label, style = MaterialTheme.typography.titleMedium)
                Text("${collection.count} 张 · ${visibilityLabel(collection.public)} · ID ${collection.id}",
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Box {
                NamedIcon(Icons.Outlined.MoreVert, "管理收藏夹 ${collection.label}", { expanded = true },
                    Modifier.testTag("collection-menu-${collection.id}"), enabled = enabled)
                DropdownMenu(expanded = expanded, onDismissRequest = { expanded = false }) {
                    DropdownMenuItem(text = { Text("编辑名称与可见性") }, enabled = enabled,
                        onClick = { expanded = false; onAction("update") },
                        modifier = Modifier.heightIn(min = 48.dp).testTag("edit-collection-${collection.id}"))
                    DropdownMenuItem(text = { Text("删除收藏夹", color = MaterialTheme.colorScheme.error) }, enabled = enabled,
                        onClick = { expanded = false; onAction("delete") },
                        modifier = Modifier.heightIn(min = 48.dp).testTag("delete-collection-${collection.id}"))
                }
            }
        }
    }
}

private fun visibilityLabel(public: Boolean?): String = when (public) {
    true -> "公开"
    false -> "私密"
    null -> "可见性未知"
}

@Composable
private fun CollectionManagementDialog(
    request: CollectionAction, locked: Boolean, onDismiss: () -> Unit, onConfirm: (String, Boolean) -> Unit
) {
    val collection = request.collection
    val deleting = request.kind == "delete"
    var label by remember(request) { mutableStateOf(collection?.label.orEmpty()) }
    // Null is intentional: an unknown website visibility must never silently become private/public.
    var public by remember(request) { mutableStateOf(if (request.kind == "create") false else collection?.public) }
    AlertDialog(
        onDismissRequest = { if (!locked) onDismiss() },
        title = { Text(when (request.kind) { "create" -> "新建收藏夹"; "delete" -> "永久删除收藏夹？"; else -> "编辑收藏夹" }) },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                if (deleting) {
                    Text("名称：${collection?.label}\nID：${collection?.id}", style = MaterialTheme.typography.titleMedium)
                    Text("这是 Wallhaven 网站上的不可撤销操作，将永久删除此收藏夹。确认无误后再删除；不会自动重试。",
                        color = MaterialTheme.colorScheme.error)
                } else {
                    OutlinedTextField(value = label, onValueChange = { label = it }, enabled = !locked,
                        label = { Text("收藏夹名称") }, singleLine = true,
                        modifier = Modifier.fillMaxWidth().testTag("collection-name"))
                    Text(if (request.kind == "create") "新收藏夹默认私密，可在下方更改。"
                        else "当前可见性：${visibilityLabel(collection?.public)}")
                    if (public == null) Text("网站未提供原可见性。请明确选择公开或私密后再保存；保存会应用你的选择。",
                        color = MaterialTheme.colorScheme.error)
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        FilterChip(selected = public == false, onClick = { public = false }, enabled = !locked,
                            label = { Text("私密") }, modifier = Modifier.heightIn(min = 48.dp).testTag("collection-private"))
                        FilterChip(selected = public == true, onClick = { public = true }, enabled = !locked,
                            label = { Text("公开") }, modifier = Modifier.heightIn(min = 48.dp).testTag("collection-public"))
                    }
                    Text("保存将直接修改网站收藏夹。", style = MaterialTheme.typography.bodySmall)
                }
            }
        },
        confirmButton = {
            TextButton(onClick = { onConfirm(label.trim(), public == true) },
                enabled = !locked && (deleting || (label.isNotBlank() && public != null)),
                modifier = Modifier.heightIn(min = 48.dp).testTag(if (deleting) "confirm-delete-collection" else "confirm-management")) {
                Text(if (deleting) "永久删除" else "确认保存", color = if (deleting) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.primary)
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss, enabled = !locked, modifier = Modifier.heightIn(min = 48.dp)) { Text("取消") }
        }
    )
}

@Composable
internal fun AccountOverlays(vm: NxViewModel, account: AccountController = vm.account) {
    val state by account.ui.collectAsStateWithLifecycle()
    // Never put a collection page in a WebView. Only the explicit website login is hosted here.
    if (state.loginOpen) AccountLogin(account)
    else if (state.favoriteOpen) AccountFavoriteSheet(account, state)
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AccountLogin(account: AccountController) {
    Dialog(onDismissRequest = { account.closeLogin() },
        properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
        Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
            Column(Modifier.fillMaxSize().safeDrawingPadding().imePadding()) {
                TopAppBar(
                    title = { Text("Wallhaven 网站登录") },
                    navigationIcon = {
                        NamedIcon(Icons.AutoMirrored.Outlined.ArrowBack, "返回并核对登录", { account.closeLogin() })
                    },
                    actions = {
                        NamedIcon(Icons.Outlined.Refresh, "重新加载登录页", { account.reloadLogin() }, Modifier.testTag("login-reload"))
                    }
                )
                Text("请在网站内手动完成登录及安全验证。密码仅在网站页面输入，本应用不展示 Cookie 或网页源码。",
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp))
                Button(onClick = { account.closeLogin() },
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp).heightIn(min = 48.dp).testTag("login-done")) {
                    Text("已完成，核对登录")
                }
                Spacer(Modifier.height(8.dp))
                AndroidView(
                    factory = { context -> FrameLayout(context).also { account.session.attach(it) } },
                    onReset = null,
                    onRelease = { account.session.detach() },
                    modifier = Modifier.fillMaxWidth().weight(1f).testTag("login-webview")
                )
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AccountFavoriteSheet(account: AccountController, state: AccountState) {
    val id = state.favoriteId
    var targetIndex by remember(id, state.favorite.targets) { mutableIntStateOf(-1) }
    var removeOpen by remember(id) { mutableStateOf(false) }
    val favoriteBusy by rememberUpdatedState(state.favoriteBusy)
    val locked = state.favoriteBusy || state.busy || state.manageBusy
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true,
        confirmValueChange = { it != SheetValue.Hidden || !favoriteBusy })
    ModalBottomSheet(
        onDismissRequest = { if (!state.favoriteBusy) account.dismissFavorite() }, sheetState = sheetState,
        modifier = Modifier.testTag("favorite-sheet"),
        dragHandle = { if (!state.favoriteBusy) BottomSheetDefaults.DragHandle() }
    ) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 24.dp).padding(bottom = 24.dp).imePadding(),
            verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text("壁纸收藏 · ${id.orEmpty()}", style = MaterialTheme.typography.titleLarge)
            Text(when {
                state.favoriteBusy -> "正在读取并核对网站收藏状态…"
                !state.favorite.known -> "收藏状态尚未确认"
                state.favorite.isFavorite -> "网站已确认：已收藏"
                else -> "网站已确认：未收藏"
            })
            if (state.favoriteBusy) CircularProgressIndicator(Modifier.size(28.dp))
            state.favoriteError?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            if (!state.favorite.known && !state.favoriteBusy) {
                Text("请刷新核对后再操作。结果未知时不要重复提交；本应用不会自动重试写操作。",
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            if (state.username == null) {
                FilledTonalButton(onClick = { account.openLogin() }, enabled = !locked,
                    modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag("favorite-login")) { Text("打开网站登录") }
            } else if (state.favorite.known && !state.favorite.isFavorite) {
                Text("选择网站提供的收藏位置", style = MaterialTheme.typography.titleSmall)
                if (state.favorite.targets.isEmpty()) Text("网站未提供可用位置，请刷新核对，或到我的收藏中新建收藏夹。")
                LazyColumn(Modifier.fillMaxWidth().heightIn(max = 240.dp)) {
                    items(state.favorite.targets.size) { index ->
                        Row(Modifier.fillMaxWidth().heightIn(min = 48.dp)
                            .clickable(enabled = !locked) { targetIndex = index }
                            .testTag("favorite-target-$index"), verticalAlignment = Alignment.CenterVertically) {
                            RadioButton(selected = targetIndex == index, onClick = { targetIndex = index }, enabled = !locked)
                            Text(state.favorite.targets[index], Modifier.weight(1f).padding(end = 8.dp))
                        }
                    }
                }
                Button(onClick = { account.setFavorite(true, targetIndex) },
                    enabled = !locked && targetIndex in state.favorite.targets.indices,
                    modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag("favorite-apply")) { Text("确认加入收藏") }
            } else if (state.favorite.known && state.favorite.isFavorite) {
                OutlinedButton(onClick = { removeOpen = true }, enabled = !locked,
                    modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag("favorite-apply")) { Text("取消收藏…") }
            }
            OutlinedButton(onClick = { id?.let { account.favorite(it) } }, enabled = !locked && id != null,
                modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag("favorite-refresh")) { Text("刷新核对收藏状态") }
            TextButton(onClick = { account.dismissFavorite() }, enabled = !state.favoriteBusy,
                modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) { Text("关闭") }
        }
    }
    if (removeOpen) AlertDialog(
        onDismissRequest = { if (!locked) removeOpen = false },
        title = { Text("取消网站收藏？") },
        text = { Text("将从 Wallhaven 网站收藏中移除壁纸 ${id.orEmpty()}。此操作会直接修改网站数据。") },
        confirmButton = {
            TextButton(onClick = { removeOpen = false; account.setFavorite(false, 0) },
                enabled = !locked && state.favorite.known && state.favorite.isFavorite,
                modifier = Modifier.heightIn(min = 48.dp).testTag("confirm-remove-favorite")) { Text("确认取消收藏") }
        },
        dismissButton = {
            TextButton(onClick = { removeOpen = false }, enabled = !locked,
                modifier = Modifier.heightIn(min = 48.dp)) { Text("保留收藏") }
        }
    )
}
