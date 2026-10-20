package io.github.nuxoul.nxwallhaven.shared

import org.json.JSONObject

data class Wall(
    val id: String,
    val original: String? = null,
    val thumbnail: String? = null,
    val width: Int = 0,
    val height: Int = 0,
    val resolution: String = "—",
    val purity: String = "sfw",
    val category: String = "—",
    val size: Long = 0,
    val format: String = "image/jpeg",
    val uploader: String? = null,
    val tags: List<Pair<Int, String>> = emptyList(),
    val colors: List<String> = emptyList(),
    val views: Long? = null,
    val favorites: Long? = null,
) {
    val aspect: Float get() = if (width > 0 && height > 0) width.toFloat() / height else 1.6f
    val filename: String get() = "wallhaven-$id." + when (format) {
        "image/png" -> "png"; "image/webp" -> "webp"; else -> "jpg"
    }
}

data class Page(val walls: List<Wall>, val page: Int, val last: Int, val seed: String?)
data class ProfileInfo(val username: String, val group: String, val biography: String, val avatar: String?, val fields: List<Pair<String, String>>)
data class CollectionInfo(val id: Int, val label: String, val count: Int, val public: Boolean? = null)
data class CacheInfo(val bytes: Long = 0, val files: Int = 0, val path: String = "")

data class Filters(
    val query: String = "",
    val categories: String = "111",
    val purity: String = "100",
    val sorting: String = "date_added",
    val order: String = "desc",
    val atleast: String = "",
    val ratios: String = "",
    val colors: String = "",
    val topRange: String = "1M",
) {
    fun json(page: Int, seed: String?): String = JSONObject().apply {
        put("q", query); put("categories", categories); put("purity", purity)
        put("sorting", sorting); put("order", order); put("page", page)
        if (atleast.isNotBlank()) put("atleast", atleast)
        if (ratios.isNotBlank()) put("ratios", ratios)
        if (colors.isNotBlank()) put("colors", colors.removePrefix("#"))
        if (sorting == "toplist") put("top_range", topRange)
        seed?.let { put("seed", it) }
    }.toString()
    fun permitted(nsfw: Boolean, hasKey: Boolean): Filters {
        val bits = purity.padEnd(3, '0').take(3).toCharArray()
        if (!nsfw || !hasKey) bits[2] = '0'
        val safe = String(bits).let { if (it == "000") "100" else it }
        return copy(purity = safe)
    }
}

object Wire {
    private fun JSONObject.text(name: String): String? = if (isNull(name)) null else optString(name).takeIf { it.isNotBlank() }
    fun wall(o: JSONObject): Wall {
        val thumbs = o.optJSONObject("thumbs")
        val tags = o.optJSONArray("tags")
        val colors = o.optJSONArray("colors")
        return Wall(
            id = o.getString("id"), original = o.text("path"),
            thumbnail = thumbs?.text("original") ?: thumbs?.text("large") ?: thumbs?.text("small"),
            width = o.optInt("dimension_x"), height = o.optInt("dimension_y"),
            resolution = o.text("resolution") ?: "—", purity = o.text("purity") ?: "sfw",
            category = o.text("category") ?: "—", size = o.optLong("file_size"),
            format = o.text("file_type") ?: "image/jpeg", uploader = o.optJSONObject("uploader")?.text("username"),
            tags = (0 until (tags?.length() ?: 0)).mapNotNull { i -> tags?.optJSONObject(i)?.let { t -> t.text("name")?.let { t.optInt("id") to it } } },
            colors = (0 until (colors?.length() ?: 0)).mapNotNull { colors?.optString(it) },
            views = if (o.isNull("views")) null else o.optLong("views"),
            favorites = if (o.isNull("favorites")) null else o.optLong("favorites"),
        )
    }
    fun page(json: String): Page {
        val o = JSONObject(json); val data = o.getJSONArray("data"); val meta = o.optJSONObject("meta") ?: JSONObject()
        return Page((0 until data.length()).map { wall(data.getJSONObject(it)) }, meta.optInt("current_page", 1), meta.optInt("last_page", 1), meta.text("seed"))
    }
    fun detail(json: String): Wall { val o = JSONObject(json); return wall(o.optJSONObject("data") ?: o) }
    fun profile(json: String): ProfileInfo {
        val o = JSONObject(json); val fields = o.optJSONArray("fields")
        return ProfileInfo(o.getString("username"), o.text("group") ?: o.text("group_name") ?: "—", o.text("biography") ?: "未公开个人简介", o.text("avatar"),
            (0 until (fields?.length() ?: 0)).mapNotNull { fields?.optJSONObject(it)?.let { f -> (f.text("label") ?: return@let null) to (f.text("value") ?: "—") } })
    }
    fun collections(json: String): List<CollectionInfo> {
        val a = JSONObject(json).getJSONArray("data")
        return (0 until a.length()).mapNotNull { a.optJSONObject(it)?.let { o -> val id=o.optInt("id"); if(id>0) CollectionInfo(id,o.text("label")?:"未命名",o.optInt("count"),if(o.isNull("public"))null else o.optBoolean("public")) else null } }
    }
    fun cache(json: String): CacheInfo { val o=JSONObject(json); return CacheInfo(o.optLong("bytes"),o.optInt("files"),o.optString("path")) }
}

/** One generation-tagged append stream; duplicates and empty pages terminate it. */
class PageAccumulator {
    var generation: Long = 0; private set
    var walls: List<Wall> = emptyList(); private set
    var page: Int = 0; private set
    var last: Int = Int.MAX_VALUE; private set
    var seed: String? = null; private set
    var loading: Boolean = false; private set
    var exhausted: Boolean = false; private set
    fun reset(): Long { generation++; walls=emptyList();page=0;last=Int.MAX_VALUE;seed=null;loading=false;exhausted=false;return generation }
    fun begin(): Pair<Long,Int>? { if(loading || exhausted) return null;loading=true;return generation to page+1 }
    fun accept(token: Long, result: Page): Boolean {
        if(token!=generation || !loading || result.page!=page+1) return false
        loading=false
        val seen=walls.mapTo(HashSet()) { it.id }
        val fresh=result.walls.filter { seen.add(it.id) }
        if(fresh.isEmpty()) { exhausted=true;return true }
        walls=walls+fresh;page=result.page;last=result.last;seed=seed?:result.seed;exhausted=page>=last;return true
    }
    fun fail(token:Long) {if(token==generation) loading=false}
}
