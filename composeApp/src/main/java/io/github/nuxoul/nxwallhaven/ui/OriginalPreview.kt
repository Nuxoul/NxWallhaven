package io.github.nuxoul.nxwallhaven.ui

import androidx.compose.foundation.layout.*
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Image
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import coil3.compose.AsyncImage
import java.io.File
import io.github.nuxoul.nxwallhaven.shared.TransferProgress

/** A thumbnail remains explicitly a preview until full-resolution tiles are displayed. */
@Composable
internal fun OriginalPreview(preview:File?,ready:Boolean,progress:TransferProgress?,stage:String,modifier:Modifier=Modifier) {
    var decoded by remember(preview){mutableStateOf(false)}
    if(!ready)Box(modifier) {
        if(preview!=null)AsyncImage(model=preview,contentDescription="原图加载中的缩略预览",contentScale=ContentScale.Fit,
            modifier=Modifier.fillMaxSize().testTag(if(decoded)"detail-thumbnail-loaded" else "detail-thumbnail"),onSuccess={decoded=true})
        else Icon(Icons.Outlined.Image,null,Modifier.align(Alignment.Center).size(44.dp),tint=MaterialTheme.colorScheme.onSurfaceVariant)
        Surface(Modifier.align(Alignment.BottomCenter).fillMaxWidth(),color=MaterialTheme.colorScheme.surface.copy(alpha=0.9f)) {
            Column(Modifier.padding(horizontal=16.dp,vertical=12.dp),verticalArrangement=Arrangement.spacedBy(8.dp)) {
                Text(if(preview!=null)"缩略预览 · 正在加载完整原图"else"正在加载完整原图",style=MaterialTheme.typography.labelMedium)
                val fraction=progress?.fraction
                if(fraction!=null)LinearProgressIndicator(progress={fraction},modifier=Modifier.fillMaxWidth().testTag("original-progress-bar"))
                else LinearProgressIndicator(Modifier.fillMaxWidth().testTag("original-progress-bar"))
                Text(buildString {
                    if(progress!=null){append("已接收 ${formatBytes(progress.received)}");progress.total?.let{append(" / ${formatBytes(it)}")}
                        progress.bytesPerSecond?.let{append(" · ${formatBytes(it)}/s")};progress.etaSeconds?.let{append(" · 剩余约 ${it}s")}}
                    else append(stage)
                },modifier=Modifier.testTag("original-transfer-progress"),style=MaterialTheme.typography.bodySmall)
            }
        }
    }
}
