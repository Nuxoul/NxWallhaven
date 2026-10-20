package io.github.nuxoul.nxwallhaven.local

import com.nxwallhaven.core.DownloadObserver
import com.nxwallhaven.core.MobileDownloads
import io.github.nuxoul.nxwallhaven.shared.DownloadItem
import io.github.nuxoul.nxwallhaven.shared.DownloadWire
import io.github.nuxoul.nxwallhaven.shared.Wall
import java.io.File
import org.json.JSONObject

/** Small adapter over the shared Rust SQLite schema. Never stores a Key or Cookie. */
class DownloadStore(cache:File,data:File) {
    private val core by lazy {MobileDownloads(cache.canonicalPath,data.canonicalPath)}
    fun recover()=core.recover()
    fun enqueue(wall:Wall):DownloadItem=DownloadWire.item(JSONObject(core.enqueue(DownloadWire.wall(wall).toString())))
    fun tasks():List<DownloadItem> = DownloadWire.items(core.tasks())
    fun library():List<DownloadItem> = DownloadWire.items(core.library())
    fun state(id:String,state:String)=core.setState(id,state)
    fun acquire(id:String,progress:(Long,Long?)->Boolean):File? {
        val path=core.acquire(id,object:DownloadObserver {
            override fun progress(received:ULong,total:ULong?):Boolean=progress(received.toLong(),total?.toLong())
        })
        return path.takeIf{it.isNotBlank()}?.let(::File)
    }
    fun complete(id:String,uri:String)=core.complete(id,uri)
    fun fail(id:String)=core.fail(id)
    fun remove(id:String,removeLibrary:Boolean)=core.remove(id,removeLibrary)
    fun missing(id:String)=core.markMissing(id)
}
