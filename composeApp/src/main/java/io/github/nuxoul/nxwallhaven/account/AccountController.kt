package io.github.nuxoul.nxwallhaven.account

import com.nxwallhaven.core.BrowserTransport
import com.nxwallhaven.core.MobileException
import com.nxwallhaven.core.MobileSite
import io.github.nuxoul.nxwallhaven.shared.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.json.JSONObject

data class FavoriteInfo(val known:Boolean=false,val isFavorite:Boolean=false,val targets:List<String> = emptyList())
data class AccountState(
    val username:String?=null,val busy:Boolean=false,val status:String="网站会话尚未连接",val loginOpen:Boolean=false,
    val collections:List<CollectionInfo> = emptyList(),val collectionsLoaded:Boolean=false,val selected:CollectionInfo?=null,
    val walls:List<Wall> = emptyList(),val purity:String="100",val loading:Boolean=false,val exhausted:Boolean=false,
    val error:String?=null,val generation:Long=0,val favoriteId:String?=null,val favorite:FavoriteInfo=FavoriteInfo(),
    val favoriteBusy:Boolean=false,val favoriteOpen:Boolean=false,val favoriteError:String?=null,
    val manageBusy:Boolean=false,val manageMessage:String?=null,
)

class AccountController(private val scope:CoroutineScope,val session:WebSession,private val permissions:()->Pair<Boolean,Boolean>, transport:BrowserTransport?=null,private val now:()->Long={android.os.SystemClock.elapsedRealtime()}) {
    private val state=MutableStateFlow(AccountState())
    val ui=state.asStateFlow()
    private val operation=Mutex()
    private val pages=PageAccumulator()
    private var epoch=0L
    private var favoriteEpoch=0L
    private data class CachedFavorite(val owner:String,val account:Long,val time:Long,val info:FavoriteInfo)
    private val favorites=LinkedHashMap<String,CachedFavorite>(32,0.75f,true)
    val collectionsLoaded:Boolean get()=state.value.collectionsLoaded
    fun ensureCollections(){if(!collectionsLoaded)refreshCollections()}
    private fun acceptOwner(owner:String) {
        if(!state.value.username.equals(owner,true)) {
            epoch++;favoriteEpoch++;favorites.clear();pages.reset()
            state.update{it.copy(username=owner,collections=emptyList(),collectionsLoaded=false,selected=null,walls=emptyList(),loading=false,exhausted=false,
                favorite=FavoriteInfo(),favoriteBusy=false,favoriteError=null,generation=pages.generation)}
        }
    }
    private val site by lazy {MobileSite(transport?:object:BrowserTransport {
        override fun request(requestJson:String):String = try {session.request(requestJson)}
        catch(_:Exception){throw MobileException.Network()}
    })}
    private suspend fun <T> call(block:()->T):T=withContext(Dispatchers.IO){block()}
    private fun message(t:Throwable):String {
        if(t is CancellationException)throw t
        return when(t){
            is MobileException.InvalidInput -> t.reason
            is MobileException.Unauthorized -> "网站会话未登录或已过期，请打开登录窗口"
            else -> "网站请求未完成，请打开登录窗口检查验证状态；写操作未自动重试"
        }
    }
    fun openLogin(){if(state.value.manageBusy || state.value.favoriteBusy)return;invalidate();state.update{it.copy(loginOpen=true,error=null)};session.login()}
    fun closeLogin(){session.detach();state.update{it.copy(loginOpen=false)};verify()}
    fun reloadLogin(){session.reload()}
    fun verify() {
        if(state.value.busy || state.value.manageBusy || state.value.favoriteBusy)return
        val token=epoch;state.update{it.copy(busy=true,status="正在核对网站会话…")}
        scope.launch{operation.withLock{if(token!=epoch)return@withLock;try{
            val owner=call{site.verify()}
            if(token!=epoch)return@withLock
            acceptOwner(owner)
            state.update{it.copy(username=owner,busy=false,status="已登录 $owner",error=null)}
            ensureCollections()
        }catch(t:Throwable){if(token==epoch){val reason=message(t);invalidate();state.update{it.copy(status=reason,error=reason)}}}}}
    }
    fun refreshCollections() {
        if(state.value.busy || state.value.manageBusy)return
        val token=epoch;state.update{it.copy(busy=true,error=null,status=if(it.username==null)"正在核对网站会话并读取收藏夹…"else"已登录 ${it.username} · 正在读取收藏夹…")}
        scope.launch{operation.withLock{if(token!=epoch)return@withLock;try{
            val json=call{site.collectionSnapshot()}
            val owner=JSONObject(json).getString("owner")
            val rows=Wire.collections(json)
            if(token!=epoch)return@withLock
            acceptOwner(owner)
            state.update{it.copy(username=owner,collections=rows,collectionsLoaded=true,busy=false,status="已登录 $owner · ${rows.size} 个收藏夹",selected=it.selected?.let{old->rows.find{it.id==old.id}})}
        }catch(t:Throwable){if(token==epoch){val reason=message(t);if(reason.contains("账号已变化") || reason.contains("未登录或已过期")){invalidate();state.update{it.copy(error=reason,status=reason)}}else state.update{it.copy(busy=false,error=reason,status=reason)}}}}}
    }
    fun openCollection(collection:CollectionInfo) {
        if(state.value.manageBusy)return
        pages.reset();state.update{it.copy(selected=collection,walls=emptyList(),loading=false,exhausted=false,error=null,generation=pages.generation)}
        loadMore()
    }
    fun closeCollection(){pages.reset();state.update{it.copy(selected=null,walls=emptyList(),loading=false,error=null,generation=pages.generation)}}
    fun filters(purity:String) {
        val (allow,key)=permissions();val safe=Filters(purity=purity).permitted(allow,key).purity
        pages.reset();state.update{it.copy(purity=safe,walls=emptyList(),loading=false,error=null,exhausted=false,generation=pages.generation)}
        if(state.value.selected!=null)loadMore()
    }
    fun refreshPage(){val selected=state.value.selected;if(selected!=null)openCollection(selected)else refreshCollections()}
    fun loadMore() {
        val snapshot=state.value;val owner=snapshot.username?:return;val selected=snapshot.selected?:return
        val (token,page)=pages.begin()?:return;val account=epoch
        state.update{it.copy(loading=true,error=null)}
        scope.launch{operation.withLock{if(account!=epoch || token!=pages.generation)return@withLock;try{
            val (allow,key)=permissions()
            val result=Wire.page(call{site.collectionPage(owner,selected.id.toUInt(),snapshot.purity,page.toUInt(),allow,key)})
            if(account==epoch && pages.accept(token,result))state.update{it.copy(walls=pages.walls,loading=false,exhausted=pages.exhausted)}
        }catch(t:Throwable){pages.fail(token);if(account==epoch && token==pages.generation){val reason=message(t);if(reason.contains("账号已变化") || reason.contains("未登录或已过期")){invalidate();state.update{it.copy(error=reason,status=reason)}}else state.update{it.copy(loading=false,error=reason)}}}}}
    }
    fun favorite(id:String,forceRefresh:Boolean=false) {
        if(!id.matches(Regex("[A-Za-z0-9]{6}")) || state.value.favoriteBusy || state.value.manageBusy)return
        val token=++favoriteEpoch;val account=epoch
        val cached=favorites[id]?.takeIf{!forceRefresh && it.account==epoch && it.owner.equals(state.value.username,true) && now()-it.time in 0L until 30_000L}
        if(cached!=null){state.update{it.copy(favoriteId=id,favoriteOpen=true,favoriteBusy=false,favorite=cached.info,favoriteError=null)};return}
        favorites.remove(id)
        state.update{it.copy(favoriteId=id,favoriteOpen=true,favoriteBusy=true,favorite=FavoriteInfo(),favoriteError=null)}
        scope.launch{operation.withLock{if(account!=epoch || token!=favoriteEpoch)return@withLock;try{
            val json=call{site.favoriteSnapshot(id)}
            val owner=JSONObject(json).getString("owner");val result=parseFavorite(json)
            if(account!=epoch || token!=favoriteEpoch)return@withLock
            acceptOwner(owner)
            favorites[id]=CachedFavorite(owner,epoch,now(),result)
            while(favorites.size>32)favorites.remove(favorites.keys.first())
            state.update{it.copy(username=owner,favoriteId=id,favoriteOpen=true,favorite=result,favoriteBusy=false,status="已登录 $owner")}
        }catch(t:Throwable){if(account==epoch && token==favoriteEpoch){val reason=message(t);if(reason.contains("未登录或已过期")){invalidate();state.update{it.copy(favoriteId=id,favoriteOpen=true,favoriteError=reason)}}else state.update{it.copy(favoriteBusy=false,favoriteError=reason)}}}}}
    }
    fun dismissFavorite(){if(!state.value.favoriteBusy){favoriteEpoch++;state.update{it.copy(favoriteOpen=false)}}}
    fun setFavorite(desired:Boolean,index:Int) {
        val snapshot=state.value;val owner=snapshot.username?:return;val id=snapshot.favoriteId?:return
        if(snapshot.favoriteBusy || !snapshot.favorite.known)return
        val label=if(desired){snapshot.favorite.targets.getOrNull(index)?:return}else ""
        if(desired && snapshot.favorite.targets.count{it==label}!=1)return
        favorites.clear()
        val token=favoriteEpoch;val account=epoch;state.update{it.copy(favoriteBusy=true,favoriteError=null)}
        scope.launch{operation.withLock{if(account!=epoch || token!=favoriteEpoch)return@withLock;try{
            val result=parseFavorite(call{site.setFavoriteTarget(owner,id,desired,label)})
            if(account==epoch && token==favoriteEpoch)state.update{it.copy(favorite=result,favoriteBusy=false,status=if(desired)"已加入收藏"else"已取消收藏")}
        }catch(t:Throwable){if(account==epoch && token==favoriteEpoch){val reason=message(t);if(reason.contains("账号已变化")){invalidate();state.update{it.copy(favoriteId=id,favoriteOpen=true,favoriteError=reason)}}else state.update{it.copy(favorite=FavoriteInfo(),favoriteBusy=false,favoriteError=reason)}}}}}
    }
    fun manage(kind:String,id:Int?,label:String,public:Boolean) {
        val snapshot=state.value;val owner=snapshot.username?:return
        if(snapshot.manageBusy || snapshot.busy || snapshot.favoriteBusy)return
        favorites.clear()
        val payload=JSONObject().put("kind",kind).apply{if(id!=null)put("id",id);if(kind!="delete"){put("label",label.trim());put("public",public)}}.toString()
        val token=epoch;state.update{it.copy(manageBusy=true,manageMessage=null,error=null)}
        scope.launch{operation.withLock{if(token!=epoch)return@withLock;try{
            val rows=Wire.collections(call{site.manageCollection(owner,payload)})
            if(token==epoch){pages.reset();state.update{it.copy(collections=rows,collectionsLoaded=true,manageBusy=false,manageMessage="操作已完成并重新读取核对",selected=null,walls=emptyList(),generation=pages.generation)}}
        }catch(t:Throwable){if(token==epoch){val reason=message(t);if(reason.contains("账号已变化") || reason.contains("未登录或已过期")){invalidate();state.update{it.copy(manageMessage=reason,error=reason)}}else state.update{it.copy(manageBusy=false,manageMessage=reason,error=reason)}}}}}
    }
    fun clearManageMessage(){state.update{it.copy(manageMessage=null)}}
    fun permissionsChanged(){favoriteEpoch++;favorites.clear();pages.reset();state.update{it.copy(walls=emptyList(),loading=false,exhausted=false,favorite=FavoriteInfo(),favoriteOpen=false,favoriteBusy=false,purity=Filters(purity=it.purity).permitted(permissions().first,permissions().second).purity,generation=pages.generation)}}
    private fun invalidate(){epoch++;favoriteEpoch++;favorites.clear();pages.reset();session.cancelPending();state.value=AccountState(status="网站会话未连接",generation=pages.generation)}
    fun logout() {
        if(state.value.manageBusy || state.value.favoriteBusy)return
        epoch++;favoriteEpoch++;favorites.clear();pages.reset();session.cancelPending();val token=epoch
        state.update{it.copy(busy=true,status="正在退出并清除本应用网站会话…")}
        scope.launch{operation.withLock{try{session.clear();if(token==epoch)state.value=AccountState(status="已退出网站；API Key、图片缓存和下载文件保留",generation=pages.generation)}catch(t:Throwable){if(token==epoch)state.update{it.copy(busy=false,error="网站登录数据未能全部清除，请重试")}}}}
    }
    private fun parseFavorite(json:String):FavoriteInfo {
        val o=JSONObject(json);val a=o.optJSONArray("targets")
        return FavoriteInfo(true,o.getBoolean("is_favorite"),(0 until(a?.length()?:0)).map{a!!.getJSONObject(it).getString("label")})
    }
}
