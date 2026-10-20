package io.github.nuxoul.nxwallhaven.shared

import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import com.nxwallhaven.core.MobileCore

/** All blocking FFI work stays off the Android main thread. */
class Repository(cacheRoot: File) {
    private val core by lazy { MobileCore(cacheRoot.canonicalPath) }
    suspend fun ping(): String = withContext(Dispatchers.IO) { core.ping() }
    suspend fun search(filters: Filters, page: Int, seed: String?, key: String?): Page = withContext(Dispatchers.IO) {
        Wire.page(core.search(filters.json(page, seed), key))
    }
    suspend fun detail(id: String, key: String?): Wall = withContext(Dispatchers.IO) { Wire.detail(core.detail(id, key)) }
    suspend fun profile(username: String): ProfileInfo = withContext(Dispatchers.IO) { Wire.profile(core.publicProfile(username)) }
    suspend fun collections(username: String): List<CollectionInfo> = withContext(Dispatchers.IO) { Wire.collections(core.collections(username, null)) }
    suspend fun collectionPage(username: String, id: Int, page: Int, purity: String, key: String?): Page = withContext(Dispatchers.IO) {
        Wire.page(core.collectionPage(username, id.toUInt(), page.toUInt(), purity, key))
    }
    suspend fun image(url: String): File = withContext(Dispatchers.IO) { File(core.acquireImage(url)) }
    suspend fun verifyApiKey(key:String) = withContext(Dispatchers.IO) { core.verifyApiKey(key) }
    /** Native callbacks run on the IO caller, never the main thread. UI observes safe counters only. */
    suspend fun imageObserved(url:String,onProgress:(TransferProgress)->Unit):File = withContext(Dispatchers.IO) {
        val meter=TransferMeter()
        File(core.acquireImageObserved(url,object:com.nxwallhaven.core.ImageObserver {
            override fun progress(received:ULong,total:ULong?) { onProgress(meter.sample(received,total)) }
        }))
    }
    suspend fun cache(): CacheInfo = withContext(Dispatchers.IO) { Wire.cache(core.cacheUsage()) }
    suspend fun clearCache() = withContext(Dispatchers.IO) { core.clearCache() }
}
