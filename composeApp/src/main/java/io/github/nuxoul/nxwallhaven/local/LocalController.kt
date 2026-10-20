package io.github.nuxoul.nxwallhaven.local

import android.app.WallpaperManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import io.github.nuxoul.nxwallhaven.shared.DownloadItem
import io.github.nuxoul.nxwallhaven.shared.Wall
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

 data class LocalState(
    val tasks:List<DownloadItem> = emptyList(),val library:List<DownloadItem> = emptyList(),val busy:Boolean=false,
    val message:String?=null,val tab:Int=0,val selected:Set<String> = emptySet(),val viewer:DownloadItem?=null,
    val query:String="",val sort:Int=0,val purity:String="all",val minWidth:Int=0,
)
class LocalController(
    private val context:Context,private val scope:CoroutineScope,private val downloads:DownloadCoordinator,
    private val launch:(Intent)->Unit,private val export:(DownloadItem)->Unit,
) {
    private val mutable=MutableStateFlow(LocalState())
    val ui=mutable.asStateFlow()
    private val mutation=Mutex()
    private var rawLibrary=emptyList<DownloadItem>()
    private var rawTasks=emptyList<DownloadItem>()
    private var refreshJob:Job?=null
    private var polling:Job?=null
    private fun filterItems(items:List<DownloadItem>,library:Boolean):List<DownloadItem> {
        val s=mutable.value
        val visible=items.filter{downloads.allowed(it.wall)}.filter {
            !library || ((s.purity=="all" || it.wall.purity==s.purity) && it.wall.width>=s.minWidth &&
                (s.query.isBlank() || (it.id+" "+it.wall.resolution+" "+it.wall.uploader.orEmpty()+" "+it.wall.tags.joinToString(" "){t->t.second}).contains(s.query,true)))
        }
        return when(s.sort){1->visible.sortedBy{it.created};2->visible.sortedByDescending{it.wall.size};else->visible.sortedByDescending{it.created}}
    }
    private fun publish(){mutable.update{it.copy(tasks=filterItems(rawTasks,false),library=filterItems(rawLibrary,true),selected=it.selected.intersect(rawLibrary.mapTo(HashSet()){r->r.id}),viewer=it.viewer?.takeIf{v->downloads.allowed(v.wall)})}}
    fun start(){if(polling?.isActive==true)return;polling=scope.launch{while(isActive){refresh();delay(800)}}}
    fun stop(){polling?.cancel();polling=null}
    fun refresh() {
        if(refreshJob?.isActive==true)return
        refreshJob=scope.launch{try{
            val result=withContext(Dispatchers.IO){downloads.initialize();downloads.store.tasks() to downloads.store.library()}
            rawTasks=result.first;rawLibrary=result.second;publish()
        }catch(_:Exception){mutable.update{it.copy(message="本地索引读取失败，请重试")}}}
    }
    fun tab(value:Int){mutable.update{it.copy(tab=value.coerceIn(0,1),selected=emptySet())};refresh()}
    fun filter(query:String,sort:Int,purity:String,minWidth:Int){mutable.update{it.copy(query=query,sort=sort.coerceIn(0,2),purity=purity,minWidth=minWidth.coerceAtLeast(0))};publish()}
    fun select(id:String){if(mutable.value.library.none{it.id==id})return;mutable.update{it.copy(selected=if(id in it.selected)it.selected-id else it.selected+id)}}
    fun clearSelection(){mutable.update{it.copy(selected=emptySet())}}
    private fun action(block:suspend ()->String?) {
        if(mutable.value.busy)return
        mutable.update{it.copy(busy=true,message=null)}
        scope.launch{mutation.withLock{try{val message=block();mutable.update{it.copy(busy=false,message=message)};refresh()}catch(_:Exception){mutable.update{it.copy(busy=false,message="操作未完成，请刷新核对；文件未被静默删除")}}}}
    }
    fun pause(id:String)=action{downloads.setState(id,"paused");"任务已暂停，可校验的临时文件保留"}
    fun resume(id:String)=action{downloads.setState(id,"queued");"任务已继续，支持时使用断点续传"}
    fun retry(id:String)=resume(id)
    fun cancel(id:String)=action{downloads.setState(id,"cancelled");"任务已取消，已发布的相册文件保留"}
    fun clearFinished()=action{withContext(Dispatchers.IO){downloads.store.tasks().filter{it.state in setOf("cancelled","failed")}.forEach{downloads.store.remove(it.id,false)}};"已清理失败/取消记录；图库文件不受影响"}
    fun enqueue(walls:List<Wall>)=action{val count=downloads.enqueue(walls);if(count>0)"已加入 $count 个下载任务，可在后台继续"else"图片已有下载记录或缺少原图地址，请在图库查看"}
    fun open(item:DownloadItem)=action{
        if(!downloads.allowed(item.wall))return@action "内容权限未开启"
        val availability=withContext(Dispatchers.IO){item.uri?.let{downloads.files.availability(it)}?:LocalFiles.Availability.MISSING}
        when(availability) {
            LocalFiles.Availability.AVAILABLE -> mutable.update{it.copy(viewer=item,selected=emptySet())}
            LocalFiles.Availability.MISSING -> {withContext(Dispatchers.IO){downloads.store.missing(item.id)};return@action "文件已移除，可在下载页重试"}
            LocalFiles.Availability.INACCESSIBLE -> return@action "暂时无权读取此图片，文件与记录已保留"
        }
        null
    }
    fun closeViewer(){mutable.update{it.copy(viewer=null)}}
    fun deleteSelected(deleteFiles:Boolean)=action{
        val selected=rawLibrary.filter{it.id in mutable.value.selected && downloads.allowed(it.wall)}
        var removed=0;var denied=0
        withContext(Dispatchers.IO){for(item in selected){
            val uri=item.uri
            val canRemove=if(!deleteFiles)true else try{uri==null || when(downloads.files.availability(uri)) {
                LocalFiles.Availability.MISSING -> true
                LocalFiles.Availability.AVAILABLE -> downloads.files.delete(uri)
                LocalFiles.Availability.INACCESSIBLE -> false
            }}catch(_:Exception){false}
            if(canRemove){downloads.store.remove(item.id,true);removed++}else denied++
        }}
        mutable.update{it.copy(selected=emptySet(),viewer=it.viewer?.takeUnless{v->selected.any{it.id==v.id}})}
        "已${if(deleteFiles)"删除"else"移除记录"} $removed 项"+(if(denied>0)"；$denied 项无删除权限，文件与记录保留，可在系统相册删除"else"")
    }
    fun share(item:DownloadItem)=action{
        check(downloads.allowed(item.wall));val uri=withContext(Dispatchers.IO){val savedUri=checkNotNull(item.uri);check(downloads.files.exists(savedUri));downloads.files.shareUri(savedUri)}
        val intent=Intent(Intent.ACTION_SEND).setType(item.wall.format).putExtra(Intent.EXTRA_STREAM,uri).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        launch(Intent.createChooser(intent,"分享本地原图"));null
    }
    fun wallpaper(item:DownloadItem,lock:Boolean)=action{
        check(downloads.allowed(item.wall));withContext(Dispatchers.IO){
            val manager=WallpaperManager.getInstance(context);check(manager.isWallpaperSupported && manager.isSetWallpaperAllowed)
            downloads.files.input(Uri.parse(item.uri!!)).use{manager.setStream(it,null,true,if(lock)WallpaperManager.FLAG_LOCK else WallpaperManager.FLAG_SYSTEM)}
        };if(lock)"锁屏壁纸已设置"else"主屏壁纸已设置"
    }
    fun export(item:DownloadItem)=action{check(downloads.allowed(item.wall));check(withContext(Dispatchers.IO){downloads.files.exists(item.uri!!)});export.invoke(item);null}
    fun permissionsChanged(credentialsChanged:Boolean=false){downloads.permissionsChanged(credentialsChanged);mutable.update{it.copy(viewer=null,selected=emptySet())};publish()}
}
