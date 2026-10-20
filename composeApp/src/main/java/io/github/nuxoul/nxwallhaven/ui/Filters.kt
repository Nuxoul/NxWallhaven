package io.github.nuxoul.nxwallhaven.ui

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import io.github.nuxoul.nxwallhaven.shared.Filters

@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
internal fun FilterSheet(initial: Filters, nsfwAllowed: Boolean, onDismiss: () -> Unit, onApply: (Filters) -> Unit) {
    var draft by remember(initial) { mutableStateOf(initial) }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(
            Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 24.dp).padding(bottom = 28.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp)
        ) {
            Text("筛选搜索结果", style = MaterialTheme.typography.headlineSmall)
            Text("分类", style = MaterialTheme.typography.titleSmall)
            BitChips(draft.categories, listOf("常规", "动漫", "人物")) { draft = draft.copy(categories = it) }
            Text("内容分级", style = MaterialTheme.typography.titleSmall)
            PurityChips(draft.purity, nsfwAllowed) { draft = draft.copy(purity = it) }
            if (!nsfwAllowed) Text("启用 NSFW 需要先保存 API 密钥，并在设置中明确允许。",
                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            ChoiceMenu("排序方式", draft.sorting, listOf(
                "date_added" to "最新", "relevance" to "相关度", "random" to "随机",
                "views" to "浏览量", "favorites" to "收藏量", "toplist" to "排行榜", "hot" to "热门"
            )) { draft = draft.copy(sorting = it) }
            ChoiceMenu("排序顺序", draft.order, listOf("desc" to "降序", "asc" to "升序")) { draft = draft.copy(order = it) }
            if (draft.sorting == "toplist") ChoiceMenu("排行周期", draft.topRange, listOf(
                "1d" to "今天", "3d" to "3 天", "1w" to "一周", "1M" to "一个月",
                "3M" to "3 个月", "6M" to "6 个月", "1y" to "一年"
            )) { draft = draft.copy(topRange = it) }
            ChoiceMenu("最低分辨率", draft.atleast, listOf(
                "" to "不限分辨率", "1920x1080" to "1920 × 1080", "2560x1440" to "2560 × 1440",
                "3840x2160" to "3840 × 2160", "7680x4320" to "7680 × 4320"
            )) { draft = draft.copy(atleast = it) }
            ChoiceMenu("宽高比", draft.ratios, listOf(
                "" to "不限比例", "16x9" to "16:9", "16x10" to "16:10", "21x9" to "21:9",
                "9x16" to "9:16 · 竖屏", "1x1" to "1:1 · 方形"
            )) { draft = draft.copy(ratios = it) }
            OutlinedTextField(
                value = draft.colors, onValueChange = { draft = draft.copy(colors = it.trim().take(7)) },
                label = { Text("颜色（可选，十六进制）") }, placeholder = { Text("#79B8FF") },
                singleLine = true, modifier = Modifier.fillMaxWidth()
            )
            val validColor = draft.colors.isBlank() || draft.colors.removePrefix("#").matches(Regex("[0-9a-fA-F]{6}"))
            if (!validColor) Text("请输入六位十六进制颜色值。", color = MaterialTheme.colorScheme.error)
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                OutlinedButton(onClick = { draft = Filters(query = initial.query) }, modifier = Modifier.weight(1f).heightIn(min = 48.dp)) { Text("重置") }
                Button(
                    onClick = { onApply(draft.permitted(nsfwAllowed, nsfwAllowed)) }, enabled = validColor,
                    modifier = Modifier.weight(1f).heightIn(min = 48.dp).testTag("apply-filters")
                ) { Text("显示壁纸") }
            }
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun PurityChips(value: String, nsfwAllowed: Boolean, onChange: (String) -> Unit) {
    BitChips(value, listOf("SFW", "Sketchy", "NSFW"), enabled = { it != 2 || nsfwAllowed }, onChange = onChange)
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun BitChips(value: String, labels: List<String>, enabled: (Int) -> Boolean = { true }, onChange: (String) -> Unit) {
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        labels.forEachIndexed { index, label ->
            val selected = value.getOrNull(index) == '1'
            FilterChip(
                selected = selected, enabled = enabled(index), label = { Text(label) },
                onClick = {
                    val bits = value.padEnd(3, '0').take(3).toCharArray()
                    bits[index] = if (selected) '0' else '1'
                    val changed = String(bits)
                    if (changed != "000") onChange(changed)
                }, modifier = Modifier.heightIn(min = 48.dp)
            )
        }
    }
}

@Composable
internal fun ChoiceMenu(label: String, value: String, choices: List<Pair<String, String>>, onChange: (String) -> Unit) {
    var expanded by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxWidth()) {
        Text(label, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Box {
            OutlinedButton(onClick = { expanded = true }, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) {
                Text(choices.firstOrNull { it.first == value }?.second ?: value)
            }
            DropdownMenu(expanded = expanded, onDismissRequest = { expanded = false }) {
                choices.forEach { (key, text) ->
                    DropdownMenuItem(text = { Text(text) }, onClick = { expanded = false; onChange(key) })
                }
            }
        }
    }
}
