package io.github.nuxoul.nxwallhaven

import android.app.Application
import android.app.WallpaperManager
import android.content.ContentValues
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import androidx.core.content.FileProvider
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.viewModelScope
import io.github.nuxoul.nxwallhaven.shared.*
import java.io.File
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** UI never receives Key/cookie values, only the explicitly safe preference summary. */
data class NxState(
    val filters:Filters=Filters(), val walls:List<Wall> = emptyList(), val loading:Boolean=false,
    val exhausted:Boolean=false,val error:String?=null,val generation:Long=0,val route:String="browse",
    val detail:Wall?=null,val detailFile:File?=null,val detailStage:String="",val detailError:String?=null,
    val actionBusy:Boolean=false,val message:String?=null,
    val profile:ProfileInfo?=null,val profileUsername:String="",val profileTab:Int=0,
    val profilePurity:String="100",val profileWalls:List<Wall> = emptyList(),val profileLoading:Boolean=false,
    val profileExhausted:Boolean=false,val profileError:String?=null,val profileGeneration:Long=0,
    val collections:List<CollectionInfo> = emptyList(),val selectedCollection:CollectionInfo?=null,
    val prefs:Prefs=Prefs(),val cache:CacheInfo=CacheInfo(),val cacheBusy:Boolean=false,
)
sealed interface PlatformEvent {
    data class Start(val intent:Intent):PlatformEvent
    data class Export(val wall:Wall,val file:File):PlatformEvent
    data class ExportLocal(val item:DownloadItem):PlatformEvent
    data object NotificationPermission:PlatformEvent
}

class NxViewModel(application:Application,private val saved:SavedStateHandle):AndroidViewModel(application) {
    private val app=application as NxApplication
    private val repository=app.repository
    private val preferences=Preferences(app)
    private val pages=PageAccumulator()
    private val profilePages=PageAccumulator()
    private data class Destination(val route:String,val detail:Wall?,val file:File?)
    private val history=ArrayDeque<Destination>()
    private var detailToken=0L
    private var profileToken=0L
    private val mutable=MutableStateFlow(NxState(filters=Filters(query=saved["query"]?:""),prefs=preferences.snapshot()))
    val ui=mutable.asStateFlow()
    val account=io.github.nuxoul.nxwallhaven.account.AccountController(viewModelScope,app.webSession,permissions={
        mutable.value.prefs.let { it.allowNsfw to it.hasKey }
    })
    fun collections(){navigate("collections");if(account.ui.value.collections.isEmpty())account.refreshCollections()}
    private val eventChannel=Channel<PlatformEvent>(Channel.BUFFERED)
    val events=eventChannel.receiveAsFlow()
    val local=io.github.nuxoul.nxwallhaven.local.LocalController(app,viewModelScope,app.downloads,
        launch={eventChannel.trySend(PlatformEvent.Start(it))},export={eventChannel.trySend(PlatformEvent.ExportLocal(it))})
    fun localPage(){navigate("local");local.refresh()}
    fun downloadBatch(walls:List<Wall>){
        viewModelScope.launch {
            try {
                val count=app.downloads.enqueue(walls)
                local.refresh()
                mutable.update{it.copy(message=if(count>0)"已加入 $count 个下载任务，可在“本地”查看进度"else"已有下载记录或原图地址不可用，请在本地页查看")}
                if(count>0)eventChannel.send(PlatformEvent.NotificationPermission)
            }catch(_:Exception){mutable.update{it.copy(message="未能加入下载队列，请重试")}}
        }
    }
    init { search(mutable.value.filters) }
    private fun key(purity:String):String? = if(purity.getOrNull(2)=='1') preferences.key() else null
    private fun failure(t:Throwable):String {
        if(t is CancellationException) throw t
        // Native errors are sanitized; never include an arbitrary exception/URL in UI or logs.
        val text=t.message.orEmpty()
        return when {
            "401" in text || "Unauthorized" in text -> "API Key 无效或没有访问此内容的权限"
            "429" in text || "RateLimited" in text -> "请求过于频繁，请稍后重试"
            "404" in text || "NotFound" in text -> "内容不存在或已被移除"
            "key" in text.lowercase() && "decrypt" in text.lowercase() -> "无法读取加密 Key，请删除后重新输入"
            else -> "加载失败，请检查网络后重试"
        }
    }
    private fun navigate(route:String) { if(ui.value.route!=route)history.addLast(Destination(ui.value.route,ui.value.detail,ui.value.detailFile));mutable.update {it.copy(route=route,error=null)} }
    fun back() {
        detailToken++
        val target=history.removeLastOrNull()?:Destination("browse",null,null)
        mutable.update {it.copy(route=target.route,detail=target.detail,detailFile=target.file,detailError=null)}
        if(target.route=="detail" && target.file?.isFile!=true) target.detail?.original?.let { loadOriginal(it,detailToken) }
    }
    fun browse() {history.clear();detailToken++;mutable.update {it.copy(route="browse",detail=null,detailFile=null)}}
    fun settings() {navigate("settings");refreshCache()}
    fun search(filters:Filters) {
        val p=ui.value.prefs
        val safe=filters.permitted(p.allowNsfw,p.hasKey)
        if(safe.categories.length!=3 || safe.categories=="000") {mutable.update {it.copy(message="请至少选择一个图片分类")};return}
        if(safe.colors.isNotBlank() && !safe.colors.removePrefix("#").matches(Regex("[A-Fa-f0-9]{6}"))) {mutable.update {it.copy(message="颜色需为 6 位十六进制值")};return}
        pages.reset();saved["query"]=safe.query
        history.clear();mutable.update {it.copy(filters=safe,walls=emptyList(),error=null,exhausted=false,loading=false,generation=pages.generation,route="browse")}
        loadMore()
    }
    fun refresh() {if(ui.value.route=="profile"){profilePages.reset();mutable.update{it.copy(profileWalls=emptyList(),profileError=null,profileExhausted=false,profileGeneration=profilePages.generation)};loadMore(true)}else search(ui.value.filters)}
    fun loadMore(profile:Boolean=false) {
        val accumulator=if(profile)profilePages else pages
        val (token,page)=accumulator.begin()?:return
        val snapshot=ui.value
        if(profile && snapshot.profileTab==2 && snapshot.selectedCollection==null){accumulator.fail(token);return}
        mutable.update {if(profile)it.copy(profileLoading=true,profileError=null)else it.copy(loading=true,error=null)}
        viewModelScope.launch {
            try {
                val purity=if(profile)snapshot.profilePurity else snapshot.filters.purity
                val auth=withContext(Dispatchers.IO){key(purity)}
                val result=if(profile && snapshot.selectedCollection!=null && snapshot.profileTab==2) {
                    repository.collectionPage(snapshot.profileUsername,snapshot.selectedCollection.id,page,purity,auth)
                } else {
                    val filters=if(profile)Filters(query="@${snapshot.profileUsername}",purity=purity) else snapshot.filters
                    repository.search(filters,page,accumulator.seed,auth)
                }
                if(accumulator.accept(token,result)) mutable.update {
                    if(profile)it.copy(profileWalls=accumulator.walls,profileLoading=false,profileExhausted=accumulator.exhausted)
                    else it.copy(walls=accumulator.walls,loading=false,exhausted=accumulator.exhausted)
                }
            }catch(t:Throwable){accumulator.fail(token);if(token==accumulator.generation)mutable.update {if(profile)it.copy(profileLoading=false,profileError=failure(t))else it.copy(loading=false,error=failure(t))}}
        }
    }
    suspend fun image(url:String):File=repository.image(url)
    private fun allowed(wall:Wall)=wall.purity!="nsfw" || (ui.value.prefs.allowNsfw && ui.value.prefs.hasKey)
    fun openDetail(wall:Wall) {
        if(!allowed(wall)){mutable.update{it.copy(message="请先配置 Key 并允许 NSFW")};return}
        navigate("detail");val token=++detailToken
        mutable.update{it.copy(detail=wall,detailFile=null,detailError=null,detailStage="正在连接原图…")}
        val known=wall.original
        if(known!=null)loadOriginal(known,token)
        viewModelScope.launch {
            try {
                val detail=repository.detail(wall.id,withContext(Dispatchers.IO){key(if(wall.purity=="nsfw")"001" else "100")})
                if(token!=detailToken)return@launch
                if(!allowed(detail)){mutable.update{it.copy(detailError="内容权限已变化",detailStage="")};return@launch}
                mutable.update{it.copy(detail=detail)}
                if(known==null)detail.original?.let{loadOriginal(it,token)}?:mutable.update{it.copy(detailError="原图地址不可用",detailStage="")}
            }catch(t:Throwable){if(token==detailToken)mutable.update{if(known==null)it.copy(detailError=failure(t))else it.copy(message="详情资料暂不可用，原图仍可正常查看")}}
        }
    }
    private fun loadOriginal(url:String,token:Long) {viewModelScope.launch {
        try {
            mutable.update{if(token==detailToken)it.copy(detailStage="正在获取完整原图文件…")else it}
            val file=repository.image(url)
            if(token==detailToken)mutable.update{it.copy(detailFile=file,detailStage="完整原图已就绪 · 双指缩放查看细节",detailError=null)}
        }catch(t:Throwable){if(token==detailToken)mutable.update{it.copy(detailStage="",detailError=failure(t))}}
    }}
    fun retryDetail(){ui.value.detail?.let{wall -> openDetail(wall)}}
    fun openProfile(username:String) {
        if(!username.matches(Regex("[A-Za-z0-9_-]{1,64}"))){mutable.update{it.copy(message="作者名称无效")};return}
        navigate("profile");val token=++profileToken;profilePages.reset()
        mutable.update{it.copy(profile=null,profileUsername=username,profileTab=0,profilePurity=it.filters.purity,profileWalls=emptyList(),collections=emptyList(),selectedCollection=null,profileError=null,profileGeneration=profilePages.generation)}
        viewModelScope.launch {try{val info=repository.profile(username);if(token==profileToken)mutable.update{it.copy(profile=info)}}catch(t:Throwable){if(token==profileToken)mutable.update{it.copy(profileError=failure(t))}}}
        viewModelScope.launch {try{val collections=repository.collections(username);if(token==profileToken)mutable.update{it.copy(collections=collections)}}catch(t:Throwable){if(token==profileToken)mutable.update{it.copy(profileError=failure(t))}}}
    }
    fun profileTab(tab:Int) {if(tab==ui.value.profileTab)return;profilePages.reset();mutable.update{it.copy(profileTab=tab,profileWalls=emptyList(),profileLoading=false,profileError=null,profileExhausted=false,profileGeneration=profilePages.generation)};if(tab==1 || (tab==2 && ui.value.selectedCollection!=null))loadMore(true)}
    fun profileFilters(purity:String) {val p=ui.value.prefs;val safe=Filters(purity=purity).permitted(p.allowNsfw,p.hasKey).purity;profilePages.reset();mutable.update{it.copy(profilePurity=safe,profileWalls=emptyList(),profileLoading=false,profileGeneration=profilePages.generation,profileExhausted=false)};if(ui.value.profileTab>0)loadMore(true)}
    fun openCollection(c:CollectionInfo) {profilePages.reset();mutable.update{it.copy(selectedCollection=c,profileTab=2,profileWalls=emptyList(),profileLoading=false,profileGeneration=profilePages.generation,profileExhausted=false)};loadMore(true)}
    fun refreshCache(){if(ui.value.cacheBusy)return;mutable.update{it.copy(cacheBusy=true)};viewModelScope.launch{try{val cache=repository.cache();mutable.update{it.copy(cache=cache,cacheBusy=false)}}catch(t:Throwable){mutable.update{it.copy(cacheBusy=false,message=failure(t))}}}}
    fun clearCache(){if(ui.value.cacheBusy)return;mutable.update{it.copy(cacheBusy=true)};viewModelScope.launch{try{repository.clearCache();val cache=repository.cache();mutable.update{it.copy(cache=cache,cacheBusy=false,message="图片缓存已清除，已保存的相册文件与设置保留")}}catch(t:Throwable){mutable.update{it.copy(cacheBusy=false,message="缓存未能全部清除，请重试")}}}}
    fun saveSettings(keyInput:String,allowNsfw:Boolean,theme:Int,grid:Boolean){viewModelScope.launch{try{val prefs=withContext(Dispatchers.IO){preferences.save(keyInput,allowNsfw,theme,grid)};applyPermissions(prefs,keyInput.isNotBlank());mutable.update{it.copy(message="设置已保存")}}catch(t:Throwable){mutable.update{it.copy(message="设置保存失败，未应用")}}}}
    fun clearKey(){viewModelScope.launch{try{val prefs=withContext(Dispatchers.IO){preferences.clearKey()};applyPermissions(prefs);mutable.update{it.copy(message="Key 已删除，NSFW 已关闭")}}catch(t:Throwable){mutable.update{it.copy(message="删除 Key 失败")}}}}
    private fun applyPermissions(prefs:Prefs,credentialsChanged:Boolean=false){
        val old=ui.value.prefs;mutable.update{it.copy(prefs=prefs)}
        if(credentialsChanged || old.allowNsfw!=prefs.allowNsfw || old.hasKey!=prefs.hasKey){
            account.permissionsChanged()
            local.permissionsChanged(credentialsChanged)
            detailToken++;profileToken++;profilePages.reset();pages.reset();history.clear()
            val safe=ui.value.filters.permitted(prefs.allowNsfw,prefs.hasKey)
            mutable.update{it.copy(filters=safe,walls=emptyList(),profileWalls=emptyList(),profileLoading=false,detail=null,detailFile=null,route="settings",generation=pages.generation,profileGeneration=profilePages.generation)}
            loadMore()
        }
    }
    private fun originalAction(block:suspend (Wall,File)->String?) {
        if(ui.value.actionBusy)return
        val wall=ui.value.detail?:return
        if(!allowed(wall))return
        val url=wall.original?:return
        val permissionEpoch=detailToken
        mutable.update{it.copy(actionBusy=true)}
        viewModelScope.launch {try{
            val file=repository.image(url) // joins an in-flight acquisition; a hit never downloads again.
            if(permissionEpoch!=detailToken || !allowed(wall)) error("权限或详情已变化")
            val message=block(wall,file)
            mutable.update{it.copy(actionBusy=false,message=message)}
        }catch(t:Throwable){mutable.update{it.copy(actionBusy=false,message="操作失败，请确认原图已加载后重试")}}}
    }
    fun saveOriginal(){
        val wall=ui.value.detail?:return
        if(!allowed(wall))return
        downloadBatch(listOf(wall))
    }
    fun exportOriginal()=originalAction {wall,file ->eventChannel.send(PlatformEvent.Export(wall,file));null}
    fun completeExport(uri:Uri?,wall:Wall,file:File){if(uri==null)return;viewModelScope.launch{try{withContext(Dispatchers.IO){app.contentResolver.openOutputStream(uri,"w")!!.use{out->file.inputStream().use{it.copyTo(out)}}};mutable.update{it.copy(message="原图已保存")}}catch(t:Throwable){mutable.update{it.copy(message="保存失败，请重新选择位置")}}}}
    fun completeLocalExport(uri:Uri?,item:DownloadItem){if(uri==null)return;viewModelScope.launch{try{withContext(Dispatchers.IO){check(app.downloads.allowed(item.wall));app.contentResolver.openOutputStream(uri,"w")!!.use{out->app.downloads.files.input(Uri.parse(item.uri!!)).use{it.copyTo(out)}}};mutable.update{it.copy(message="本地原图已导出")}}catch(_:Exception){mutable.update{it.copy(message="本地导出失败，原文件保留")}}}}
    fun shareOriginal()=originalAction {wall,file ->
        val uri=FileProvider.getUriForFile(app,"${app.packageName}.files",file)
        val intent=Intent(Intent.ACTION_SEND).setType(wall.format).putExtra(Intent.EXTRA_STREAM,uri).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        eventChannel.send(PlatformEvent.Start(Intent.createChooser(intent,"分享原图")));null
    }
    fun wallpaper(lock:Boolean)=originalAction {_,file ->withContext(Dispatchers.IO){
        val manager=WallpaperManager.getInstance(app)
        check(manager.isWallpaperSupported && manager.isSetWallpaperAllowed){"系统不允许设置壁纸"}
        file.inputStream().use{manager.setStream(it,null,true,if(lock)WallpaperManager.FLAG_LOCK else WallpaperManager.FLAG_SYSTEM)}
        if(lock)"锁屏壁纸已设置"else"主屏壁纸已设置"
    }}
    fun clearMessage(){mutable.update{it.copy(message=null)}}
    fun platformFailure(){mutable.update{it.copy(message="没有可处理此操作的系统应用")}}
}
