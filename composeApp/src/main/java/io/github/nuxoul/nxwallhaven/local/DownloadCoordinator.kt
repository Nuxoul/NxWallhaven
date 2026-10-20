package io.github.nuxoul.nxwallhaven.local

import android.content.Context
import io.github.nuxoul.nxwallhaven.Preferences
import io.github.nuxoul.nxwallhaven.shared.Wall
import kotlinx.coroutines.*
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/** One owner per application process; recovery is performed once, before any worker runs. */
class DownloadCoordinator(private val context:Context) {
    val store=DownloadStore(context.cacheDir,context.filesDir)
    val files=LocalFiles(context)
    private val scope=CoroutineScope(SupervisorJob()+Dispatchers.IO)
    private val gate=Mutex()
    private var initialized=false
    val publicationLock=Any()
    private val permissionChanges=java.util.concurrent.atomic.AtomicInteger(0)
    init {scope.launch{runCatching{initialize();if(store.tasks().any{it.state=="queued"})DownloadWorker.schedule(context)}}}
    suspend fun initialize()=gate.withLock{if(!initialized){withContext(Dispatchers.IO){store.recover()};initialized=true}}
    fun allowed(wall:Wall):Boolean=wall.purity!="nsfw" || (permissionChanges.get()==0 && Preferences(context).snapshot().let{it.allowNsfw && it.hasKey})
    suspend fun enqueue(walls:List<Wall>):Int=withContext(Dispatchers.IO){
        initialize();var count=0
        for(wall in walls.distinctBy{it.id}) {
            if(!allowed(wall) || wall.original==null)continue
            var item=store.enqueue(wall)
            val savedUri=item.uri
            if(item.complete && savedUri!=null && files.availability(savedUri)==LocalFiles.Availability.MISSING){store.missing(item.id);item=store.enqueue(wall)}
            if(item.state=="queued")count++
        }
        if(count>0)DownloadWorker.schedule(context)
        count
    }
    suspend fun setState(id:String,state:String)=withContext(Dispatchers.IO){
        initialize()
        val item=store.tasks().find{it.id==id}?:return@withContext
        if(state=="queued" && !allowed(item.wall))error("内容权限未开启，任务保持暂停")
        synchronized(publicationLock) {
            store.state(id,state)
            if(state=="cancelled" && store.tasks().find{it.id==id}?.state=="cancelled")files.discardPending(id)
        }
        if(state=="queued")DownloadWorker.schedule(context)
    }
    fun permissionsChanged(credentialsChanged:Boolean=false){
        permissionChanges.incrementAndGet()
        scope.launch {try {
            initialize()
            synchronized(publicationLock){store.tasks().filter{it.active && it.wall.purity=="nsfw" && (credentialsChanged || !allowed(it.wall))}.forEach{store.state(it.id,"paused")}}
        }finally{permissionChanges.decrementAndGet()}}
    }
}
