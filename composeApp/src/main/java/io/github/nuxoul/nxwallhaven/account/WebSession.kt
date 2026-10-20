package io.github.nuxoul.nxwallhaven.account

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Bitmap
import android.net.Uri
import android.net.http.SslError
import android.os.Handler
import android.os.Looper
import android.view.ViewGroup
import android.webkit.CookieManager
import android.webkit.SslErrorHandler
import android.webkit.WebResourceRequest
import android.webkit.WebSettings
import android.webkit.WebStorage
import android.webkit.WebView
import android.webkit.WebViewClient
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.locks.ReentrantLock
import org.json.JSONArray
import org.json.JSONObject

/** Only this WebView owns the website session; no Cookie/password export or JS bridge. */
class WebSession(private val context:Context, private val origin:String="https://wallhaven.cc", private val fixture:Boolean=false) {
    private val main=Handler(Looper.getMainLooper())
    private val serial=ReentrantLock()
    private val generation=AtomicLong()
    private var view:WebView?=null
    private var loading=false
    private var documentCommitted=false
    private var pageEpoch=0L
    private var securityError=false
    private var pendingCancel:(()->Unit)?=null
    private val limit=2*1024*1024
    init {require(origin=="https://wallhaven.cc" || (fixture && origin.matches(Regex("http://127\\.0\\.0\\.1:[0-9]+"))))}
    private fun sameOrigin(url:String?):Boolean=runCatching {
        val a=Uri.parse(url?:return false);val b=Uri.parse(origin)
        a.scheme==b.scheme && a.host==b.host && a.port==b.port && a.userInfo==null
    }.getOrDefault(false)
    private fun requireMain(){check(Looper.myLooper()==Looper.getMainLooper())}
    @SuppressLint("SetJavaScriptEnabled")
    @Suppress("DEPRECATION")
    fun webView():WebView {
        requireMain()
        view?.let{return it}
        return WebView(context.applicationContext).apply {
            settings.javaScriptEnabled=true
            settings.domStorageEnabled=true
            settings.allowFileAccess=false
            settings.allowContentAccess=false
            settings.mixedContentMode=WebSettings.MIXED_CONTENT_NEVER_ALLOW
            settings.setSupportMultipleWindows(false)
            settings.saveFormData=false
            importantForAutofill=android.view.View.IMPORTANT_FOR_AUTOFILL_NO_EXCLUDE_DESCENDANTS
            CookieManager.getInstance().setAcceptCookie(true)
            CookieManager.getInstance().setAcceptThirdPartyCookies(this,false)
            webViewClient=object:WebViewClient(){
                override fun shouldOverrideUrlLoading(v:WebView,request:WebResourceRequest):Boolean {
                    return request.isForMainFrame && !sameOrigin(request.url.toString())
                }
                override fun onPageStarted(v:WebView,url:String?,favicon:Bitmap?) {
                    loading=true;pageEpoch++;pendingCancel?.invoke();pendingCancel=null
                    documentCommitted=false
                    securityError=!sameOrigin(url)
                }
                override fun onPageCommitVisible(v:WebView,url:String?) {
                    if(sameOrigin(url) && sameOrigin(v.url))documentCommitted=true
                }
                override fun onPageFinished(v:WebView,url:String?) {
                    loading=false;securityError=securityError || !sameOrigin(url)
                    documentCommitted=sameOrigin(url)
                    CookieManager.getInstance().flush()
                }
                override fun onReceivedSslError(v:WebView,handler:SslErrorHandler,error:SslError) {
                    securityError=true;handler.cancel();pendingCancel?.invoke();pendingCancel=null
                }
            }
            view=this
            loading=true
            loadUrl("$origin/")
        }
    }
    fun attach(parent:ViewGroup) {
        requireMain();val browser=webView();(browser.parent as? ViewGroup)?.removeView(browser)
        parent.addView(browser,ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT,ViewGroup.LayoutParams.MATCH_PARENT))
    }
    fun detach(){requireMain();view?.let{(it.parent as? ViewGroup)?.removeView(it);CookieManager.getInstance().flush()}}
    fun login(){requireMain();val browser=webView();if(!sameOrigin(browser.url) || Uri.parse(browser.url).path!="/login")browser.loadUrl("$origin/login")}
    fun reload(){requireMain();webView().reload()}
    fun cancelPending(){generation.incrementAndGet();main.post{pendingCancel?.invoke();pendingCancel=null}}

    /** Called on a Rust worker, never on UI thread. Unknown POST outcome is not retried. */
    fun request(json:String):String {
        check(Looper.myLooper()!=Looper.getMainLooper()){"网站请求不能阻塞界面线程"}
        serial.lock()
        try {
            val input=JSONObject(json)
            val method=input.getString("method").uppercase()
            val target=input.getString("url")
            check(method=="GET" || method=="POST")
            // Rust site parser emits canonical exact-origin absolute URLs only.
            check(sameOrigin(target) && !target.contains('\\') && Uri.parse(target).fragment==null)
            val csrf=input.optString("csrf").takeUnless{input.isNull("csrf") || it.isBlank()}
            val fields=input.optJSONArray("fields")?:JSONArray()
            check(method!="POST" || csrf!=null)
            check(method=="POST" || (csrf==null && fields.length()==0))
            val result=java.util.concurrent.atomic.AtomicReference<String?>()
            val latch=CountDownLatch(1)
            val epoch=generation.get()
            val deadline=android.os.SystemClock.uptimeMillis()+32_000
            val key="nxwh_"+UUID.randomUUID().toString().replace("-","")
            var browser:WebView?=null
            var capturedPage=0L
            fun fail(){latch.countDown()}
            lateinit var poll:Runnable
            fun inspect(raw:String?) {
                if(latch.count==0L)return
                if(epoch!=generation.get() || pageEpoch!=capturedPage || securityError || !sameOrigin(browser?.url)){fail();return}
                if(raw==null || raw=="null") {
                    if(android.os.SystemClock.uptimeMillis()>deadline)fail() else main.postDelayed(poll,60)
                    return
                }
                try {
                    val envelope=JSONObject(raw)
                    if(envelope.optBoolean("error")){fail();return}
                    val output=envelope.getJSONObject("response")
                    val body=output.getString("body")
                    check(body.toByteArray(Charsets.UTF_8).size<=limit)
                    val url=output.getString("url");check(sameOrigin(url))
                    val status=output.getInt("status");check(status in 100..599)
                    result.set(output.toString());latch.countDown()
                }catch(_:Exception){fail()}
            }
            poll=Runnable {
                val v=browser
                if(latch.count==0L)return@Runnable
                if(v==null || epoch!=generation.get() || pageEpoch!=capturedPage || securityError || !sameOrigin(v.url)){fail();return@Runnable}
                v.evaluateJavascript("(()=>{if(location.origin!==${JSONObject.quote(origin)})return {error:true};return window[${JSONObject.quote(key)}]??null})()",::inspect)
            }
            lateinit var start:Runnable
            start=Runnable {
                if(epoch!=generation.get() || android.os.SystemClock.uptimeMillis()>deadline){fail();return@Runnable}
                val v=webView();browser=v
                // Login HTML can be usable while a page image/subresource never finishes.
                // CommitVisible proves this is the new main document, not the previous page.
                if(loading && !documentCommitted){main.postDelayed(start,80);return@Runnable}
                if(securityError || !sameOrigin(v.url)){fail();return@Runnable}
                capturedPage=pageEpoch
                pendingCancel=::fail
                val script="""
                    (async()=>{
                      const expected=${JSONObject.quote(origin)},key=${JSONObject.quote(key)},limit=$limit;
                      const finish=(value)=>{window[key]=value;};
                      try {
                        if(location.origin!==expected)throw Error('origin');
                        const target=${JSONObject.quote(target)},method=${JSONObject.quote(method)},csrf=${if(csrf==null)"null"else JSONObject.quote(csrf)},fields=$fields;
                        if(new URL(target).origin!==expected)throw Error('origin');
                        const headers={'Accept':method==='POST'?'application/json':'text/html,application/xhtml+xml'};
                        if(method==='POST'){headers['X-CSRF-TOKEN']=csrf;headers['X-Requested-With']='XMLHttpRequest';headers['Content-Type']='application/x-www-form-urlencoded;charset=UTF-8';}
                        const abort=new AbortController(),timer=setTimeout(()=>abort.abort(),25000);
                        try{
                          const r=await fetch(target,{method,headers,body:method==='POST'?new URLSearchParams(fields).toString():undefined,credentials:'same-origin',redirect:'error',signal:abort.signal});
                          if(new URL(r.url).origin!==expected)throw Error('origin');
                          const reader=r.body?.getReader(),decoder=new TextDecoder();let body='',bytes=0;
                          if(reader){while(true){const part=await reader.read();if(part.done)break;bytes+=part.value.byteLength;if(bytes>limit){await reader.cancel();throw Error('limit');}body+=decoder.decode(part.value,{stream:true});}body+=decoder.decode();}
                          finish({response:{status:r.status,url:r.url,body}});
                        }finally{clearTimeout(timer);}
                      }catch(_){finish({error:true});}
                    })();null;
                """.trimIndent()
                v.evaluateJavascript(script){main.post(poll)}
            }
            main.post(start)
            try {if(!latch.await(35,TimeUnit.SECONDS))fail()}catch(interrupted:InterruptedException){Thread.currentThread().interrupt();throw IllegalStateException("网站请求已取消")}
            main.post {
                main.removeCallbacks(start);main.removeCallbacks(poll)
                if(epoch==generation.get()){pendingCancel=null;browser?.evaluateJavascript("delete window[${JSONObject.quote(key)}]",null)}
            }
            return result.get()?:throw IllegalStateException(if(method=="POST")"操作结果未确认，请刷新核对；未自动重试"else"网站请求未完成，请打开登录窗口检查验证状态")
        } finally {serial.unlock()}
    }
    suspend fun clear()=kotlinx.coroutines.suspendCancellableCoroutine<Unit>{continuation->
        cancelPending()
        main.post {
            val browser=view
            browser?.clearCache(true)
            browser?.let{(it.parent as? ViewGroup)?.removeView(it);it.stopLoading();it.destroy()}
            view=null;loading=false;documentCommitted=false;pageEpoch++
            CookieManager.getInstance().removeAllCookies {
                CookieManager.getInstance().flush();WebStorage.getInstance().deleteAllData()
                securityError=false
                if(continuation.isActive)continuation.resume(Unit){_,_,_->}
            }
        }
    }
    fun destroy(){requireMain();cancelPending();view?.let{(it.parent as? ViewGroup)?.removeView(it);it.destroy()};view=null;documentCommitted=false;loading=false}
}
