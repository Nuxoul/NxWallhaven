package io.github.nuxoul.nxwallhaven

import android.os.Process
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.nxwallhaven.core.BrowserTransport
import com.nxwallhaven.core.MobileException
import com.nxwallhaven.core.MobileSite
import io.github.nuxoul.nxwallhaven.account.WebSession
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

/** Host fixture survives the app process; no Cookie read/export is used. */
@RunWith(AndroidJUnit4::class)
class WebSessionRestartProbe {
    private val app get()=ApplicationProvider.getApplicationContext<NxApplication>()
    private val args get()=InstrumentationRegistry.getArguments()
    private val origin get()="http://127.0.0.1:${args.getString("sessionPort")!!.toInt()}"
    private fun main(block:()->Unit)=InstrumentationRegistry.getInstrumentation().runOnMainSync(block)
    @Before fun onlyExplicitEmulatorProbe(){
        org.junit.Assume.assumeTrue(args.getString("restartProbe")=="true" && args.containsKey("sessionPort"))
        org.junit.Assume.assumeTrue(android.os.Build.MODEL.contains("sdk") || android.os.Build.FINGERPRINT.contains("generic"))
    }
    private fun site(session:WebSession)=MobileSite(object:BrowserTransport {
        override fun request(requestJson:String):String {
            val request=JSONObject(requestJson);val target=request.getString("url")
            check(target.startsWith("https://wallhaven.cc/"))
            request.put("url",origin+target.removePrefix("https://wallhaven.cc"))
            val response=try{JSONObject(session.request(request.toString()))}catch(_:Exception){throw MobileException.Network()}
            response.put("url","https://wallhaven.cc"+response.getString("url").removePrefix(origin))
            return response.toString()
        }
    })
    private fun waitPage(session:WebSession,path:String){
        repeat(100){
            val latch=CountDownLatch(1);var ready=false
            main{session.webView().evaluateJavascript("location.pathname===${JSONObject.quote(path)}&&document.readyState==='complete'"){ready=it=="true";latch.countDown()}}
            latch.await(1,TimeUnit.SECONDS);if(ready)return
            Thread.sleep(100)
        };error("restart fixture page unavailable")
    }
    @Test fun prepare()=runBlocking {
        val session=WebSession(app,origin,true)
        try {
            session.clear();main{session.login()};waitPage(session,"/login")
            main{session.webView().evaluateJavascript("document.getElementById('signin').click()",null)}
            waitPage(session,"/fixture-login")
            assertEquals("RestartFixture",site(session).verify())
            File(app.filesDir,"web-session-test-first.pid").writeText(Process.myPid().toString())
        }finally{main{session.detach();session.destroy()}}
    }
    @Test fun restoreAndClear()=runBlocking {
        assertNotEquals(File(app.filesDir,"web-session-test-first.pid").readText(),Process.myPid().toString())
        val session=WebSession(app,origin,true)
        try {
            assertEquals("RestartFixture",site(session).verify())
            session.clear()
            assertTrue(runCatching{site(session).verify()}.isFailure)
        }finally{main{session.destroy()}}
    }
}
