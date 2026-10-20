package io.github.nuxoul.nxwallhaven

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.nxwallhaven.core.BrowserTransport
import com.nxwallhaven.core.MobileException
import io.github.nuxoul.nxwallhaven.account.AccountController
import io.github.nuxoul.nxwallhaven.account.WebSession
import kotlinx.coroutines.*
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

/** Mock browser callback, real UniFFI MobileSite/parser; never contacts wallhaven. */
@RunWith(AndroidJUnit4::class)
class AccountRequestCountTest {
    private class Browser : BrowserTransport {
        val calls = java.util.Collections.synchronizedList(mutableListOf<String>())
        @Volatile var owner = "Alice"
        @Volatile var favorite = false
        @Volatile var empty = false
        @Volatile var drop = false
        @Volatile var reverse = false
        override fun request(requestJson: String): String {
            val r = JSONObject(requestJson)
            val path = r.getString("url").removePrefix("https://wallhaven.cc")
            val method = r.getString("method")
            calls.add("$method $path")
            val panel = """<div id="userpanel"><a href="/user/$owner">$owner</a><form action="/logout"></form></div>"""
            val body = if (method == "POST") {
                assertEquals("fresh", r.getString("csrf"))
                if (drop) throw MobileException.Network()
                favorite = path.contains("/add/")
                """{"status":true}"""
            } else if (path == "/favorites") {
                panel + """<ul class="collections-list">${if(empty) "" else """<li class="collection"><a href="/favorites/12"><span class="collection-label">Default</span></a></li>"""}</ul>"""
            } else if (path.startsWith("/w/")) {
                val a = """<a class="add-fav" href="/favorite/add/abc123/12">Default</a>"""
                val b = """<a class="add-fav" href="/favorite/add/abc123/13">Other</a>"""
                panel + """<meta name="csrf-token" content="fresh">""" +
                    if(favorite) """<a id="fav-button" href="/favorite/remove/abc123">Remove</a>"""
                    else """<div id="fav-button" class="add-button">${if(reverse)b+a else a+b}</div>"""
            } else panel
            return JSONObject().put("status",200).put("url",r.getString("url")).put("body",body).toString()
        }
    }
    private fun main(block: () -> Unit) = InstrumentationRegistry.getInstrumentation().runOnMainSync(block)
    private fun waitFor(done: () -> Boolean) {
        val until = System.currentTimeMillis()+10_000
        while(!done() && System.currentTimeMillis()<until) Thread.sleep(10)
        assertTrue("controller did not settle",done())
    }
    @Test fun snapshotsCacheForceExpiryEmptyAndFreshWrites() {
        val browser=Browser()
        val scope=CoroutineScope(SupervisorJob()+Dispatchers.Main.immediate)
        val session=WebSession(InstrumentationRegistry.getInstrumentation().targetContext.applicationContext)
        var clock=0L
        val account=AccountController(scope,session,{false to false},browser,{clock})
        try {
            fun collections(){ main{account.refreshCollections()};waitFor{!account.ui.value.busy} }
            fun favorite(force:Boolean=false){main{account.favorite("abc123",force)};waitFor{!account.ui.value.favoriteBusy} }
            collections();assertEquals(listOf("GET /favorites"),browser.calls.toList())
            browser.calls.clear();collections();assertEquals(listOf("GET /favorites"),browser.calls.toList())
            browser.calls.clear();favorite();assertEquals(listOf("GET /w/abc123"),browser.calls.toList())
            browser.calls.clear();main{account.dismissFavorite()};favorite();assertTrue(browser.calls.isEmpty())
            favorite(true);assertEquals(listOf("GET /w/abc123"),browser.calls.toList())
            browser.calls.clear();clock=30_001;favorite();assertEquals(listOf("GET /w/abc123"),browser.calls.toList())
            browser.calls.clear();browser.reverse=true
            main{account.setFavorite(true,0)};waitFor{!account.ui.value.favoriteBusy}
            assertEquals(listOf("GET /w/abc123","POST /favorite/add/abc123/12","GET /w/abc123"),browser.calls.toList())
            assertTrue(account.ui.value.favorite.isFavorite)
            browser.calls.clear();browser.owner="Bob"
            main{account.setFavorite(false,0)};waitFor{!account.ui.value.favoriteBusy}
            assertEquals(listOf("GET /w/abc123"),browser.calls.toList())
            assertFalse(account.ui.value.favorite.known)
            browser.calls.clear();favorite(true);browser.calls.clear();browser.drop=true
            main{account.setFavorite(false,0)};waitFor{!account.ui.value.favoriteBusy}
            assertEquals(listOf("GET /w/abc123","POST /favorite/remove/abc123"),browser.calls.toList())
            assertFalse(account.ui.value.favorite.known)
            browser.drop=false;browser.empty=true;collections();browser.calls.clear()
            main{account.ensureCollections()};Thread.sleep(100);assertTrue(browser.calls.isEmpty())
            // At most 32 wallpaper states survive; the least-recently-used one must be read again.
            for(i in 0..32){val id="a"+i.toString().padStart(5,'0');main{account.favorite(id)};waitFor{!account.ui.value.favoriteBusy}}
            browser.calls.clear();main{account.favorite("a00000")};waitFor{!account.ui.value.favoriteBusy}
            assertEquals(listOf("GET /w/a00000"),browser.calls.toList())
            browser.calls.clear();main{account.permissionsChanged();account.favorite("a00000")};waitFor{!account.ui.value.favoriteBusy}
            assertEquals(listOf("GET /w/a00000"),browser.calls.toList())
        } finally {scope.cancel();main{session.destroy()}}
    }
}
