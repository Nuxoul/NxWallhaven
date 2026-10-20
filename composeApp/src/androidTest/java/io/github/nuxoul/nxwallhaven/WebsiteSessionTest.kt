package io.github.nuxoul.nxwallhaven

import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.lazy.staggeredgrid.rememberLazyStaggeredGridState
import androidx.compose.material3.MaterialTheme
import androidx.lifecycle.ViewModelProvider
import io.github.nuxoul.nxwallhaven.account.AccountController
import io.github.nuxoul.nxwallhaven.ui.AccountScreen
import io.github.nuxoul.nxwallhaven.ui.AccountOverlays
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import android.webkit.CookieManager
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.nxwallhaven.core.BrowserTransport
import com.nxwallhaven.core.MobileException
import com.nxwallhaven.core.MobileSite
import io.github.nuxoul.nxwallhaven.account.WebSession
import io.github.nuxoul.nxwallhaven.shared.Wire
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class WebsiteSessionTest {
    @get:Rule val compose=createAndroidComposeRule<MainActivity>()
    private val app get()=compose.activity.application as NxApplication
    private fun onMain(block:()->Unit)=InstrumentationRegistry.getInstrumentation().runOnMainSync(block)
    private fun waitPage(session:WebSession,path:String) {
        val deadline=System.currentTimeMillis()+15_000
        while(System.currentTimeMillis()<deadline){
            val latch=java.util.concurrent.CountDownLatch(1);var ready=false
            onMain{val v=session.webView();v.evaluateJavascript("location.pathname===${JSONObject.quote(path)} && document.readyState==='complete'"){ready=it=="true";latch.countDown()}}
            latch.await(2,java.util.concurrent.TimeUnit.SECONDS)
            if(ready)return
            Thread.sleep(100)
        }
        error("fixture page did not become ready")
    }
    private fun bridge(session:WebSession,origin:String)=MobileSite(transport(session,origin))
    private fun transport(session:WebSession,origin:String)=object:BrowserTransport {
        override fun request(requestJson:String):String {
            val input=JSONObject(requestJson)
            val canonical=input.getString("url");check(canonical.startsWith("https://wallhaven.cc/"))
            input.put("url",origin+canonical.removePrefix("https://wallhaven.cc"))
            val response=try{JSONObject(session.request(input.toString()))}catch(_:Exception){throw MobileException.Network()}
            check(response.getString("url").startsWith(origin+"/"))
            response.put("url","https://wallhaven.cc"+response.getString("url").removePrefix(origin))
            return response.toString()
        }
    }
    @Test fun httpOnlySessionPrivateCollectionFreshWritesAndExplicitLogout()=runBlocking {
        org.junit.Assume.assumeTrue(android.os.Build.MODEL.contains("sdk") || android.os.Build.FINGERPRINT.contains("generic"))
        SiteFixture().use{fixture->
            var session=WebSession(app,fixture.origin,true)
            session.clear()
            onMain{session.login()};waitPage(session,"/login")
            var site=bridge(session,fixture.origin)
            assertTrue(runCatching{site.verify()}.isFailure)
            // Submitting the local fixture login form, never the real website/password.
            onMain{session.webView().evaluateJavascript("document.getElementById('signin').click()",null)}
            waitPage(session,"/fixture-login")
            assertEquals(fixture.username,site.verify())
            val collections=Wire.collections(site.collections(fixture.username))
            assertEquals(false,collections.first{it.id==12}.public)
            assertEquals("Default",collections.first{it.id==12}.label)
            val content=Wire.page(site.collectionPage(fixture.username,12u,"100",1u,false,false))
            assertEquals(listOf("abc123"),content.walls.map{it.id})
            assertTrue(runCatching{site.collectionPage(fixture.username,12u,"001",1u,false,false)}.isFailure)
            assertTrue(runCatching{site.collections("OtherUser")}.isFailure)
            assertFalse(JSONObject(site.favoriteState("abc123")).getBoolean("is_favorite"))
            assertTrue(JSONObject(site.setFavorite(fixture.username,"abc123",true,0u)).getBoolean("is_favorite"))
            assertEquals(1,fixture.posts.get())
            site.setFavorite(fixture.username,"abc123",true,0u)
            assertEquals(1,fixture.posts.get())
            assertFalse(JSONObject(site.setFavorite(fixture.username,"abc123",false,0u)).getBoolean("is_favorite"))
            fixture.dropNextWrite.set(true)
            val created=Wire.collections(site.manageCollection(fixture.username,"""{"kind":"create","label":"Created","public":false}"""))
            assertEquals("Created",created.first{it.id==14}.label)
            assertEquals(3,fixture.posts.get())
            site.manageCollection(fixture.username,"""{"kind":"update","id":14,"label":"Renamed","public":true}""")
            assertEquals("Renamed",fixture.rows.first{it.id==14}.label)
            site.manageCollection(fixture.username,"""{"kind":"delete","id":14}""")
            assertFalse(fixture.rows.any{it.id==14})
            assertEquals(5,fixture.posts.get())
            assertFalse(fixture.paths.any{it.startsWith("/user/") && it.contains("favorites")})
            assertFalse(fixture.paths.any{it.endsWith("/delete") && it !in listOf("/collections/14/delete")})
            // Cookie cannot be read by page JS but survives WebView recreation.
            val js=java.util.concurrent.atomic.AtomicReference<String>();val latch=java.util.concurrent.CountDownLatch(1)
            onMain{session.webView().evaluateJavascript("document.cookie"){js.set(it);latch.countDown()}}
            latch.await(3,java.util.concurrent.TimeUnit.SECONDS);assertFalse(js.get().contains("nxfixture"))
            onMain{CookieManager.getInstance().flush();session.destroy()}
            session=WebSession(app,fixture.origin,true);site=bridge(session,fixture.origin)
            assertEquals(fixture.username,site.verify())
            val oversized=JSONObject().put("method","GET").put("url",fixture.origin+"/too-large").put("fields",org.json.JSONArray()).put("csrf",JSONObject.NULL)
            assertTrue(runCatching{session.request(oversized.toString())}.isFailure)
            session.clear()
            assertTrue(runCatching{bridge(session,fixture.origin).verify()}.isFailure)
            onMain{session.destroy()}
        }
    }
    @Test fun nativeAccountNavigationAndLoginCloseRetainsBrowse() {
        compose.onNodeWithTag("nav-collections").performClick()
        compose.onNodeWithTag("account").assertIsDisplayed()
        compose.waitUntil(40_000){!compose.onNodeWithTag("account-login").fetchSemanticsNode().config.contains(androidx.compose.ui.semantics.SemanticsProperties.Disabled)}
        compose.onNodeWithTag("account-login").performClick()
        compose.onNodeWithTag("login-webview").assertExists()
        compose.onNodeWithTag("login-done").performClick()
        compose.onNodeWithTag("account").assertIsDisplayed()
        compose.onNodeWithTag("nav-discover").performClick()
        compose.onNodeWithTag("gallery").assertIsDisplayed()
    }
    @Test fun nativeFavoriteAndCollectionManagementConfirmations()=runBlocking {
        org.junit.Assume.assumeTrue(android.os.Build.MODEL.contains("sdk") || android.os.Build.FINGERPRINT.contains("generic"))
        SiteFixture().use{fixture ->
            val session=WebSession(app,fixture.origin,true)
            val scope=CoroutineScope(SupervisorJob()+Dispatchers.Main.immediate)
            try {
                session.clear();onMain{session.login()};waitPage(session,"/login")
                onMain{session.webView().evaluateJavascript("document.getElementById('signin').click()",null)}
                waitPage(session,"/fixture-login")
                val account=AccountController(scope,session,{false to false},transport(session,fixture.origin))
                onMain {
                    val vm=ViewModelProvider(compose.activity)[NxViewModel::class.java]
                    compose.activity.setContent { MaterialTheme { Box {
                        AccountScreen(vm,rememberLazyStaggeredGridState(),true,account)
                        AccountOverlays(vm,account)
                    } } }
                    account.refreshCollections()
                }
                compose.waitUntil(30_000){account.ui.value.collections.size==2 && !account.ui.value.busy}
                assertEquals(false,account.ui.value.collections.first{it.id==12}.public)
                androidx.test.uiautomator.UiDevice.getInstance(InstrumentationRegistry.getInstrumentation()).takeScreenshot(java.io.File(app.getExternalFilesDir(null),"account-local-collections.png"))
                onMain{account.favorite("abc123")}
                compose.waitUntil(20_000){!account.ui.value.favoriteBusy}
                compose.onNodeWithTag("favorite-target-0").performClick()
                compose.onNodeWithTag("favorite-apply").performClick()
                compose.waitUntil(20_000){!account.ui.value.favoriteBusy && account.ui.value.favorite.isFavorite}
                compose.onNodeWithTag("favorite-apply").performClick()
                compose.onNodeWithTag("confirm-remove-favorite").performClick()
                compose.waitUntil(20_000){!account.ui.value.favoriteBusy && !account.ui.value.favorite.isFavorite}
                onMain{account.dismissFavorite()}
                compose.onNodeWithTag("new-collection").performScrollTo().performClick()
                compose.onNodeWithTag("collection-name").performTextInput("Native created")
                compose.onNodeWithTag("confirm-management").performClick()
                compose.waitUntil(20_000){!account.ui.value.manageBusy && account.ui.value.collections.any{it.id==14}}
                assertFalse(account.ui.value.collections.first{it.id==14}.public!!)
                compose.onNodeWithTag("collection-14").performScrollTo()
                compose.onNodeWithTag("collection-menu-14").performClick()
                compose.onNodeWithTag("delete-collection-14").performClick()
                compose.onNodeWithTag("confirm-delete-collection").performClick()
                compose.waitUntil(20_000){!account.ui.value.manageBusy && account.ui.value.collections.none{it.id==14}}
                assertEquals(4,fixture.posts.get())
                session.clear()
            } finally {onMain{scope.cancel();session.destroy()}}
        }
    }
    @Test fun queuedFavoriteInvalidationDoesNotLeaveTheSheetLocked() {
        val scope=CoroutineScope(SupervisorJob()+Dispatchers.Main.immediate)
        val entered=java.util.concurrent.CountDownLatch(1)
        val release=java.util.concurrent.CountDownLatch(1)
        val calls=java.util.concurrent.atomic.AtomicInteger()
        val session=WebSession(app)
        val account=AccountController(scope,session,{false to false},object:BrowserTransport {
            override fun request(requestJson:String):String {
                if(calls.incrementAndGet()==1){entered.countDown();check(release.await(5,java.util.concurrent.TimeUnit.SECONDS))}
                val request=JSONObject(requestJson);check(request.getString("method")=="GET")
                val body="""<div id="userpanel"><a href="/user/Test">Test</a><form action="/logout"></form></div><div id="collections"></div>"""
                return JSONObject().put("status",200).put("url",request.getString("url")).put("body",body).toString()
            }
        })
        try {
            onMain{account.verify()}
            assertTrue(entered.await(5,java.util.concurrent.TimeUnit.SECONDS))
            onMain{account.favorite("abc123")}
            assertTrue(account.ui.value.favoriteBusy)
            release.countDown()
            compose.waitUntil(10_000){!account.ui.value.busy}
            assertFalse("invalidated favorite left a permanently locked sheet",account.ui.value.favoriteBusy)
            onMain{account.dismissFavorite()}
            assertFalse(account.ui.value.favoriteOpen)
        } finally {release.countDown();onMain{scope.cancel();session.destroy()}}
    }
    @Test fun loginDoneRecognizesSessionWithoutWaitingForPageImages()=runBlocking {
        org.junit.Assume.assumeTrue(android.os.Build.MODEL.contains("sdk") || android.os.Build.FINGERPRINT.contains("generic"))
        SiteFixture().use {fixture ->
            fixture.stallAfterLogin.set(true)
            val session=WebSession(app,fixture.origin,true)
            val scope=CoroutineScope(SupervisorJob()+Dispatchers.Main.immediate)
            val account=AccountController(scope,session,{false to false},transport(session,fixture.origin))
            try {
                session.clear()
                onMain {
                    val vm=ViewModelProvider(compose.activity)[NxViewModel::class.java]
                    compose.activity.setContent {MaterialTheme {Box {
                        AccountScreen(vm,rememberLazyStaggeredGridState(),true,account)
                        AccountOverlays(vm,account)
                    }}}
                    account.openLogin()
                }
                waitPage(session,"/login")
                onMain{session.webView().evaluateJavascript("document.getElementById('signin').click()",null)}
                assertTrue("fixture did not stall the post-login image",fixture.slowResourceStarted.await(5,java.util.concurrent.TimeUnit.SECONDS))
                compose.onNodeWithTag("login-done").performClick()
                val end=System.currentTimeMillis()+3_000
                while(account.ui.value.username==null && System.currentTimeMillis()<end)Thread.sleep(50)
                assertEquals("login confirmation waited for an unrelated page image instead of recognizing the authenticated session",fixture.username,account.ui.value.username)
                compose.waitUntil(5_000){!account.ui.value.busy}
                assertEquals(2,account.ui.value.collections.size)
            }finally{fixture.releaseSlowResource.countDown();onMain{scope.cancel();session.destroy()}}
        }
    }
}
