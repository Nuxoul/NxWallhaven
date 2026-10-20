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
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.first
import org.json.JSONObject
import org.json.JSONArray
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit

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
    val detailProgress:TransferProgress?=null,val keyChecking:Boolean=false,val keyStatus:String="尚未验证（保存密钥不代表有效）",
    val detailPreview:File?=null,
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
    @Volatile private var detailToken=0L
    private var profileToken=0L
    private val initialPrefs=preferences.snapshot()
    private val restoredFilters=Filters.restore(saved.get<Any>("filters-v1") as? String,initialPrefs.allowNsfw,initialPrefs.hasKey)
    private val mutable=MutableStateFlow(NxState(filters=restoredFilters,prefs=initialPrefs))
    val ui=mutable.asStateFlow()
    val account=io.github.nuxoul.nxwallhaven.account.AccountController(viewModelScope,app.webSession,permissions={
        mutable.value.prefs.let { it.allowNsfw to it.hasKey }
    })
    fun collections(){navigate("collections");account.ensureCollections()}
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
    private val thumbnailGate=Semaphore(4)
    private val recentThumbnails=object:LinkedHashMap<String,File>(128,0.75f,true){override fun removeEldestEntry(eldest:MutableMap.MutableEntry<String,File>?)=size>128}
    init {
        restoreNavigation()
        viewModelScope.launch { ui.collect { persistNavigation() } }
        viewModelScope.launch { local.ui.collect { s ->
            saved["local-v1"]=JSONObject().put("tab",s.tab).put("query",s.query.take(4096)).put("sort",s.sort)
                .put("purity",s.purity).put("width",s.minWidth).put("viewer",s.viewer?.id).toString()
        } }
    }
    private fun persistNavigation() {
        val s=ui.value
        saved["filters-v1"]=s.filters.savedJson()
        saved["navigation-v1"]=JSONObject().put("version",1).put("route",s.route).put("detail",s.detail?.id)
            .put("username",s.profileUsername).put("tab",s.profileTab).put("purity",s.profilePurity)
            .put("collection",s.selectedCollection?.id).put("history",JSONArray().apply {
                history.takeLast(12).forEach { put(JSONObject().put("route",it.route).put("detail",it.detail?.id)) }
            }).toString()
    }
    private fun restoreNavigation() {
        val raw=saved.get<Any>("navigation-v1") as? String
        val nav=runCatching {
            require(raw!=null && raw.length<=8192)
            JSONObject(raw).also { o ->
                val fields=setOf("version","route","detail","username","tab","purity","collection","history")
                require(o.opt("version")==1 && o.keys().asSequence().all { it in fields })
                require(o.opt("route") is String)
                for(name in listOf("detail","username","purity"))require(!o.has(name) || o.opt(name) is String)
                for(name in listOf("tab","collection"))require(!o.has(name) || o.opt(name) is Int)
                require(!o.has("history") || o.opt("history") is JSONArray)
            }
        }.getOrNull()
        fun route(value:String)=value.takeIf { it in setOf("browse","settings","profile","detail","local","collections") }?:"browse"
        fun id(value:String)=value.takeIf { it.matches(Regex("[A-Za-z0-9]{6}")) }
        val target=route(nav?.optString("route").orEmpty())
        val user=nav?.optString("username").orEmpty().takeIf { it.matches(Regex("[A-Za-z0-9_-]{1,64}")) }
        val tab=nav?.optInt("tab",0)?.coerceIn(0,2)?:0
        val collection=nav?.optInt("collection",0)?.takeIf { it>0 }
        if(user!=null && (target=="profile" || target=="detail" || nav?.optJSONArray("history")?.toString()?.contains("profile")==true)) {
            openProfile(user)
            val purity=Filters(purity=nav?.optString("purity")?:"100").permitted(initialPrefs.allowNsfw,initialPrefs.hasKey).purity
            mutable.update { it.copy(profileTab=tab,profilePurity=purity,selectedCollection=collection?.let { c->CollectionInfo(c,"收藏集",0) }) }
            if(tab==1 || (tab==2 && collection!=null))loadMore(true)
        }
        history.clear()
        nav?.optJSONArray("history")?.let { a -> for(i in 0 until minOf(a.length(),12)) {
            val entry=a.optJSONObject(i)?:continue
            val r=route(entry.optString("route"));val wall=id(entry.optString("detail"))?.let { Wall(it) }
            if(r!="profile" || user!=null)history.addLast(Destination(r,wall,null))
        } }
        mutable.update { it.copy(route=if(target=="profile" && user==null)"browse" else target) }
        when(target) {
            "detail" -> id(nav?.optString("detail").orEmpty())?.let { openDetail(Wall(it)) }?:browse()
            "settings" -> refreshCache()
            "local" -> restoreLocal()
            "collections" -> account.ensureCollections()
        }
        if(ui.value.route=="browse" || history.any { it.route=="browse" }) {pages.reset();mutable.update { it.copy(generation=pages.generation) };loadMore()}
        persistNavigation()
    }
    private fun restoreLocal() {
        val raw=saved.get<Any>("local-v1") as? String
        val o=runCatching {
            require(raw!=null && raw.length<=8192)
            JSONObject(raw).also { json ->
                val fields=setOf("tab","query","sort","purity","width","viewer")
                require(json.keys().asSequence().all { it in fields })
                for(name in listOf("query","purity","viewer"))require(!json.has(name) || json.opt(name) is String)
                for(name in listOf("tab","sort","width"))require(!json.has(name) || json.opt(name) is Int)
            }
        }.getOrNull()
        local.tab(o?.optInt("tab",0)?:0)
        val purity=o?.optString("purity")?.takeIf { it in setOf("all","sfw","sketchy","nsfw") }?:"all"
        local.filter(o?.optString("query").orEmpty().take(4096),o?.optInt("sort",0)?:0,
            if(purity=="nsfw" && !(initialPrefs.allowNsfw && initialPrefs.hasKey))"all" else purity,
            (o?.optInt("width",0)?:0).coerceIn(0,100000))
        val viewer=o?.optString("viewer")?.takeIf { it.matches(Regex("[A-Za-z0-9]{6}")) }?:return
        val epoch=detailToken
        viewModelScope.launch { val s=kotlinx.coroutines.withTimeoutOrNull(15_000) { local.ui.first { it.library.any { item->item.id==viewer } } }?:return@launch
            if(ui.value.route=="local" && epoch==detailToken)s.library.firstOrNull { it.id==viewer }?.let { local.open(it) }
        }
    }
    private val presentedGrids=HashSet<Boolean>()
    fun consumeGridRestoration(profile:Boolean):Pair<Int,Int>?=if(presentedGrids.add(profile))gridPosition(profile)else null
    fun gridPosition(profile:Boolean):Pair<Int,Int> {
        val key=if(profile)"profile-grid-v1" else "browse-grid-v1"
        val p=saved.get<Any>(key) as? IntArray
        return (p?.getOrNull(0)?:0).coerceIn(0,100000) to (p?.getOrNull(1)?:0).coerceIn(0,100000)
    }
    fun saveGridPosition(profile:Boolean,index:Int,offset:Int) {
        saved[if(profile)"profile-grid-v1" else "browse-grid-v1"]=intArrayOf(index.coerceIn(0,100000),offset.coerceIn(0,100000))
    }
    private fun key(purity:String):String? = if(purity.getOrNull(2)=='1') preferences.key() else null
    private fun failure(t:Throwable):String {
        if(t is CancellationException) throw t
        when(t) {
            is com.nxwallhaven.core.MobileException.Unauthorized -> return "API Key 无效或没有访问此内容的权限"
            is com.nxwallhaven.core.MobileException.RateLimited -> return "请求过于频繁，请稍后重试"
            is com.nxwallhaven.core.MobileException.NotFound -> return "内容不存在或已被移除"
            is com.nxwallhaven.core.MobileException.InvalidInput -> return "请求参数或密钥格式无效，请检查后重试"
            else -> Unit
        }
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
    fun detailBaseRoute():String=history.lastOrNull { it.route!="detail" }?.route?:"browse"
    private fun navigate(route:String) { if(ui.value.route!=route)history.addLast(Destination(ui.value.route,ui.value.detail,ui.value.detailFile));mutable.update {it.copy(route=route,error=null)} }
    fun back() {
        detailToken++
        val target=history.removeLastOrNull()?:Destination("browse",null,null)
        mutable.update {it.copy(route=target.route,detail=target.detail,detailFile=target.file,detailError=null)}
        if(target.route=="detail" && target.file?.isFile!=true)target.detail?.let { openDetail(it) }
        if(target.route=="browse" && pages.page==0 && !pages.loading)loadMore()
    }
    fun browse() {history.clear();detailToken++;mutable.update {it.copy(route="browse",detail=null,detailFile=null)};if(pages.page==0 && !pages.loading)loadMore()}
    fun settings() {navigate("settings");refreshCache()}
    fun search(filters:Filters) {
        val p=ui.value.prefs
        val safe=filters.permitted(p.allowNsfw,p.hasKey)
        if(safe.categories.length!=3 || safe.categories=="000") {mutable.update {it.copy(message="请至少选择一个图片分类")};return}
        if(safe.colors.isNotBlank() && !safe.colors.removePrefix("#").matches(Regex("[A-Fa-f0-9]{6}"))) {mutable.update {it.copy(message="颜色需为 6 位十六进制值")};return}
        if(!safe.valid()){mutable.update { it.copy(message="筛选条件无效，请检查后重试") };return}
        pages.reset();saveGridPosition(false,0,0)
        history.clear();detailToken++;mutable.update {it.copy(filters=safe,walls=emptyList(),error=null,exhausted=false,loading=false,generation=pages.generation,route="browse",detail=null,detailFile=null)}
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
    suspend fun image(url:String):File=repository.image(url).also { synchronized(recentThumbnails){recentThumbnails[url]=it} }
    suspend fun prefetchThumbnails(walls:List<Wall>)=coroutineScope {
        walls.mapNotNull{it.thumbnail}.distinct().take(72).forEach { url -> launch {
            thumbnailGate.withPermit {
                if(synchronized(recentThumbnails){recentThumbnails.containsKey(url)})return@withPermit
                try{image(url)}catch(cancelled:CancellationException){throw cancelled}catch(_:Exception){/* Visible image retains its normal retry path. */}
            }
        } }
    }
    private fun allowed(wall:Wall)=!wall.purity.equals("nsfw",true) || (ui.value.prefs.allowNsfw && ui.value.prefs.hasKey)
    fun openDetail(wall:Wall) {
        if(!allowed(wall)){mutable.update{it.copy(message="请先配置 Key 并允许 NSFW")};return}
        navigate("detail");val token=++detailToken
        val preview=wall.thumbnail?.let{url->synchronized(recentThumbnails){recentThumbnails[url]}}
        mutable.update{it.copy(detail=wall,detailFile=null,detailPreview=preview,detailError=null,detailStage="正在连接原图…",detailProgress=null)}
        if(preview==null)wall.thumbnail?.let{url->viewModelScope.launch{try{val file=image(url);if(token==detailToken)mutable.update{it.copy(detailPreview=file)}}catch(cancelled:CancellationException){throw cancelled}catch(_:Exception){}}}
        val known=wall.original
        if(known!=null)loadOriginal(known,token)
        viewModelScope.launch {
            try {
                val detail=repository.detail(wall.id,withContext(Dispatchers.IO){key(if(ui.value.prefs.allowNsfw && ui.value.prefs.hasKey)"001" else "100")})
                if(token!=detailToken)return@launch
                if(!allowed(detail)){mutable.update{it.copy(detailError="内容权限已变化",detailStage="")};return@launch}
                mutable.update{it.copy(detail=detail.copy(thumbnail=detail.thumbnail?:wall.thumbnail))}
                if(known==null)detail.original?.let{loadOriginal(it,token)}?:mutable.update{it.copy(detailError="原图地址不可用",detailStage="")}
            }catch(t:Throwable){if(token==detailToken)mutable.update{if(known==null)it.copy(detailError=failure(t))else it.copy(message="详情资料暂不可用，原图仍可正常查看")}}
        }
    }
    private fun loadOriginal(url:String,token:Long) {viewModelScope.launch {
        try {
            mutable.update{if(token==detailToken)it.copy(detailStage="正在获取完整原图文件…")else it}
            val file=repository.imageObserved(url) { progress ->
                // The native thread only publishes immutable state. Stale callbacks cannot reveal old content.
                mutable.update { if(token==detailToken)it.copy(detailProgress=progress)else it }
            }
            if(token==detailToken)mutable.update{it.copy(detailFile=file,detailStage="完整原图已就绪 · 双指缩放查看细节",detailError=null)}
        }catch(t:Throwable){if(token==detailToken)mutable.update{it.copy(detailStage="",detailError=failure(t))}}
    }}
    fun retryDetail(){ui.value.detail?.let{wall -> openDetail(wall)}}
    fun openProfile(username:String) {
        if(!username.matches(Regex("[A-Za-z0-9_-]{1,64}"))){mutable.update{it.copy(message="作者名称无效")};return}
        navigate("profile");val token=++profileToken;profilePages.reset()
        mutable.update{it.copy(profile=null,profileUsername=username,profileTab=0,profilePurity=it.filters.purity,profileWalls=emptyList(),profileLoading=false,profileExhausted=false,collections=emptyList(),selectedCollection=null,profileError=null,profileGeneration=profilePages.generation)}
        viewModelScope.launch {try{val info=repository.profile(username);if(token==profileToken)mutable.update{it.copy(profile=info)}}catch(t:Throwable){if(token==profileToken)mutable.update{it.copy(profileError=failure(t))}}}
        viewModelScope.launch {try{val collections=repository.collections(username);if(token==profileToken)mutable.update{it.copy(collections=collections)}}catch(t:Throwable){if(token==profileToken)mutable.update{it.copy(profileError=failure(t))}}}
    }
    fun profileTab(tab:Int) {if(tab !in 0..2 || tab==ui.value.profileTab)return;profilePages.reset();saveGridPosition(true,0,0);mutable.update{it.copy(profileTab=tab,profileWalls=emptyList(),profileLoading=false,profileError=null,profileExhausted=false,profileGeneration=profilePages.generation)};if(tab==1 || (tab==2 && ui.value.selectedCollection!=null))loadMore(true)}
    fun profileFilters(purity:String) {val p=ui.value.prefs;val safe=Filters(purity=purity).permitted(p.allowNsfw,p.hasKey).purity;profilePages.reset();mutable.update{it.copy(profilePurity=safe,profileWalls=emptyList(),profileLoading=false,profileGeneration=profilePages.generation,profileExhausted=false)};if(ui.value.profileTab>0)loadMore(true)}
    fun openCollection(c:CollectionInfo) {profilePages.reset();mutable.update{it.copy(selectedCollection=c,profileTab=2,profileWalls=emptyList(),profileLoading=false,profileGeneration=profilePages.generation,profileExhausted=false)};loadMore(true)}
    private var keyCheckToken=0L
    fun keyInputChanged() {keyCheckToken++;mutable.update { it.copy(keyChecking=false,keyStatus="尚未验证（保存密钥不代表有效）") }}
    fun verifyKey(input:String) {
        if(ui.value.keyChecking)return
        val token=++keyCheckToken
        mutable.update { it.copy(keyChecking=true,keyStatus="正在向 API 服务器验证…") }
        viewModelScope.launch {
            val status=try {
                val candidate=withContext(Dispatchers.IO) { input.trim().takeIf { it.isNotEmpty() }?:preferences.key() }
                if(candidate==null)"请先输入或保存 API 密钥" else {repository.verifyApiKey(candidate);"API 服务器已确认密钥有效（仅本次验证）"}
            } catch(t:Throwable) { failure(t) }
            if(token==keyCheckToken)mutable.update { it.copy(keyChecking=false,keyStatus=status) }
        }
    }
    fun refreshCache(){if(ui.value.cacheBusy)return;mutable.update{it.copy(cacheBusy=true)};viewModelScope.launch{try{val cache=repository.cache();mutable.update{it.copy(cache=cache,cacheBusy=false)}}catch(t:Throwable){mutable.update{it.copy(cacheBusy=false,message=failure(t))}}}}
    fun clearCache(){if(ui.value.cacheBusy)return;mutable.update{it.copy(cacheBusy=true)};viewModelScope.launch{try{repository.clearCache();synchronized(recentThumbnails){recentThumbnails.clear()};val cache=repository.cache();mutable.update{it.copy(cache=cache,cacheBusy=false,message="图片缓存已清除，已保存的相册文件与设置保留")}}catch(t:Throwable){mutable.update{it.copy(cacheBusy=false,message="缓存未能全部清除，请重试")}}}}
    fun saveSettings(keyInput:String,allowNsfw:Boolean,theme:Int,grid:Boolean){viewModelScope.launch{try{val prefs=withContext(Dispatchers.IO){preferences.save(keyInput,allowNsfw,theme,grid)};applyPermissions(prefs,keyInput.isNotBlank());mutable.update{it.copy(message="设置已保存")}}catch(t:Throwable){mutable.update{it.copy(message="设置保存失败，未应用")}}}}
    fun clearKey(){viewModelScope.launch{try{val prefs=withContext(Dispatchers.IO){preferences.clearKey()};applyPermissions(prefs);mutable.update{it.copy(message="Key 已删除，NSFW 已关闭")}}catch(t:Throwable){mutable.update{it.copy(message="删除 Key 失败")}}}}
    private fun applyPermissions(prefs:Prefs,credentialsChanged:Boolean=false){
        val old=ui.value.prefs;mutable.update{it.copy(prefs=prefs)}
        if(credentialsChanged || old.allowNsfw!=prefs.allowNsfw || old.hasKey!=prefs.hasKey){
            account.permissionsChanged()
            local.permissionsChanged(credentialsChanged)
            detailToken++;profileToken++;profilePages.reset();pages.reset();history.clear();keyInputChanged()
            val safe=ui.value.filters.permitted(prefs.allowNsfw,prefs.hasKey)
            mutable.update{it.copy(filters=safe,walls=emptyList(),profile=null,collections=emptyList(),selectedCollection=null,profilePurity=Filters(purity=it.profilePurity).permitted(prefs.allowNsfw,prefs.hasKey).purity,profileWalls=emptyList(),profileLoading=false,detail=null,detailFile=null,detailPreview=null,detailProgress=null,detailError=null,detailStage="",route="settings",generation=pages.generation,profileGeneration=profilePages.generation)}
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
