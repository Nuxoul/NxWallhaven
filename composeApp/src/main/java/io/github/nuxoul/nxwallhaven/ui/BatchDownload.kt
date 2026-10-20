package io.github.nuxoul.nxwallhaven.ui

import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Download
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import io.github.nuxoul.nxwallhaven.NxViewModel
import io.github.nuxoul.nxwallhaven.shared.Wall

/** Confirmation freezes only the loaded list; never silently fetches more pages. */
@Composable
internal fun BatchDownloadButton(vm:NxViewModel,walls:List<Wall>) {
    var pending by remember { mutableStateOf<List<Wall>?>(null) }
    NamedIcon(Icons.Outlined.Download,"批量下载已加载图片",{pending=walls.distinctBy{it.id}.toList()},
        Modifier.testTag("batch-download"),enabled=walls.isNotEmpty())
    pending?.let { snapshot ->
        AlertDialog(onDismissRequest={pending=null},title={Text("下载已加载的 ${snapshot.size} 张图片？")},
            text={Text("仅将当前已加载的图片加入后台队列，不会继续抓取其他页面。已完成的图片不会重复保存；原图会占用设备存储。")},
            confirmButton={TextButton(onClick={pending=null;vm.downloadBatch(snapshot)},modifier=Modifier.testTag("confirm-batch-download")){Text("加入下载队列")}},
            dismissButton={TextButton(onClick={pending=null}){Text("取消")}})
    }
}
