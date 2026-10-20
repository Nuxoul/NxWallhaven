package io.github.nuxoul.nxwallhaven

import android.graphics.BitmapFactory
import android.provider.MediaStore
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.lifecycle.ViewModelProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.UiDevice
import io.github.nuxoul.nxwallhaven.shared.*
import java.io.File
import java.security.MessageDigest
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

/** Read-only public API calls; all file writes stay in the dedicated emulator. */
@RunWith(AndroidJUnit4::class)
class AndroidAcceptanceTest {
    @get:Rule val compose=createAndroidComposeRule<MainActivity>()
    private val device get()=UiDevice.getInstance(InstrumentationRegistry.getInstrumentation())
    private val app get()=compose.activity.application as NxApplication
    private lateinit var vm:NxViewModel
    private fun model():NxViewModel {
        compose.runOnUiThread{vm=ViewModelProvider(compose.activity)[NxViewModel::class.java]}
        return vm
    }
    private fun waitFor(timeout:Long=90_000,condition:()->Boolean) {compose.waitUntil(timeout){condition()}}
    private fun screenshot(name:String) {device.takeScreenshot(File(app.getExternalFilesDir(null),"acceptance-$name.png"))}
    private fun digest(file:File):ByteArray=file.inputStream().use{stream->val md=MessageDigest.getInstance("SHA-256");val b=ByteArray(65536);while(true){val n=stream.read(b);if(n<0)break;md.update(b,0,n)};md.digest()}
    private fun shell(command:String) {InstrumentationRegistry.getInstrumentation().uiAutomation.executeShellCommand(command).use{it.fileDescriptor.let {fd->java.io.FileInputStream(fd).use{input->input.readBytes()}}}}
    @Before fun restoreEmulatorNetwork() {
        org.junit.Assume.assumeTrue("acceptance writes/network switches are emulator-only", android.os.Build.FINGERPRINT.contains("generic") || android.os.Build.MODEL.contains("sdk"))
        Preferences(app).save("",false,1,false)
        val current=model();compose.runOnUiThread{current.saveSettings("",false,1,false)}
        waitFor{current.ui.value.prefs.theme==1 && !current.ui.value.prefs.grid}
        shell("svc wifi enable"); shell("svc data enable")
        val manager=app.getSystemService(android.net.ConnectivityManager::class.java)
        val deadline=System.currentTimeMillis()+45_000
        while(System.currentTimeMillis()<deadline) {
            if(manager.getNetworkCapabilities(manager.activeNetwork)?.hasCapability(android.net.NetworkCapabilities.NET_CAPABILITY_VALIDATED)==true)return
            Thread.sleep(500)
        }
        error("emulator network did not validate after restoration")
    }

    @Test fun realSearchPaginationOriginalAndReturn() {
        val model=model();compose.runOnUiThread{model.search(Filters())};waitFor{model.ui.value.walls.isNotEmpty() || model.ui.value.error!=null}
        if(model.ui.value.error!=null) runBlocking { try {app.repository.search(Filters(),1,null,null)} catch(t:Exception){throw AssertionError("search error type=${t.javaClass.simpleName}")} }
        assertNull("real search failed",model.ui.value.error)
        val before=model.ui.value.walls.size
        compose.runOnUiThread{model.loadMore()}
        waitFor{!model.ui.value.loading}
        assertTrue("next page did not append",model.ui.value.walls.size>before)
        compose.onNodeWithTag("gallery").assertIsDisplayed()
        compose.waitUntil(90_000){compose.onAllNodesWithTag("image-loaded",useUnmergedTree=true).fetchSemanticsNodes().size>=3}
        screenshot("gallery")
        val first=model.ui.value.walls[10]
        compose.onNodeWithTag("wallpaper-grid").performScrollToIndex(10)
        val beforeTop=compose.onNodeWithTag("card-${first.id}").fetchSemanticsNode().boundsInRoot.top
        compose.onNodeWithTag("card-${first.id}").performClick()
        waitFor(180_000){model.ui.value.detailFile!=null || model.ui.value.detailError!=null}
        assertNotNull("original file missing: ${model.ui.value.detailError}",model.ui.value.detailFile)
        compose.waitUntil(60_000){compose.onAllNodesWithText("原图已显示",substring=true).fetchSemanticsNodes().isNotEmpty()}
        compose.onNodeWithTag("original-image").assertIsDisplayed()
        screenshot("original")
        val original=model.ui.value.detailFile!!
        val dimensions=BitmapFactory.Options().apply{inJustDecodeBounds=true;BitmapFactory.decodeFile(original.absolutePath,this)}
        assertTrue(dimensions.outWidth>0 && dimensions.outHeight>0)
        compose.activityRule.scenario.recreate()
        compose.onNodeWithTag("detail").assertIsDisplayed()
        compose.onNodeWithTag("detail-back").performClick()
        compose.onNodeWithTag("card-${first.id}").assertExists()
        val afterTop=compose.onNodeWithTag("card-${first.id}").fetchSemanticsNode().boundsInRoot.top
        assertTrue("return lost nonzero scroll position",kotlin.math.abs(beforeTop-afterTop)<8f)
        assertTrue(model.ui.value.walls.size>before)
    }

    @Test fun nativeProfileUploadsAndCollections() {
        val model=model()
        compose.runOnUiThread{model.openProfile("whimsicalmango")}
        waitFor{model.ui.value.profile!=null || model.ui.value.profileError!=null}
        assertNotNull("native profile failed: ${model.ui.value.profileError}",model.ui.value.profile)
        compose.onNodeWithTag("profile").assertIsDisplayed()
        screenshot("profile")
        compose.onNodeWithTag("profile-tab-1").performClick()
        waitFor{model.ui.value.profileWalls.isNotEmpty() || model.ui.value.profileError!=null}
        assertTrue("uploads missing: ${model.ui.value.profileError}",model.ui.value.profileWalls.isNotEmpty())
        val before=model.ui.value.profileWalls.size
        compose.runOnUiThread{model.loadMore(true)}
        waitFor{!model.ui.value.profileLoading}
        assertTrue(model.ui.value.profileWalls.size>=before)
        waitFor{model.ui.value.collections.isNotEmpty()}
        val collection=model.ui.value.collections.first()
        compose.runOnUiThread{model.openCollection(collection)}
        waitFor{!model.ui.value.profileLoading}
        assertNull(model.ui.value.profileError)
        assertEquals(collection.id,model.ui.value.selectedCollection?.id)
        compose.waitUntil(90_000){compose.onAllNodesWithTag("image-loaded",useUnmergedTree=true).fetchSemanticsNodes().isNotEmpty()}
        screenshot("collection")
    }

    @Test fun keystoreCiphertextBlankRetentionAndExplicitRemoval() {
        val prefs=Preferences(app)
        val fake="android-fixture-not-a-real-wallhaven-key"
        try {
            prefs.save(fake,false,1,false)
            assertEquals(fake,prefs.key())
            val raw=File(app.applicationInfo.dataDir,"shared_prefs/settings-v1.xml").readText()
            assertFalse("plaintext key persisted",raw.contains(fake))
            prefs.save("",false,2,true)
            assertEquals(fake,Preferences(app).key())
            assertTrue(prefs.snapshot().grid)
            assertFalse(prefs.snapshot().allowNsfw)
        } finally {prefs.clearKey()}
        assertNull(prefs.key());assertFalse(prefs.snapshot().allowNsfw)
    }

    @Test fun cachedOriginalOfflineExportAndSafeClear()=runBlocking {
        check(android.os.Build.FINGERPRINT.contains("generic") || android.os.Build.MODEL.contains("sdk")) {"Offline test is emulator-only"}
        val root=File(app.cacheDir,"acceptance-cache").apply{mkdirs()}
        val repository=Repository(root)
        val url="https://w.wallhaven.cc/full/w5/wallhaven-w5mqr6.jpg"
        val cached=repository.image(url)
        val expected=digest(cached)
        val original=app.repository.image(url)
        val model=model()
        try {
            shell("svc wifi disable");shell("svc data disable")
            val reopened=Repository(root).image(url)
            assertArrayEquals(expected,digest(reopened))
            compose.runOnUiThread{model.openDetail(Wall(id="w5mqr6",original=url,width=6240,height=4160,resolution="6240x4160"))}
            waitFor{model.ui.value.detailFile!=null}
            compose.runOnUiThread{model.saveOriginal()}
            waitFor{!model.ui.value.actionBusy}
            assertTrue("save did not complete offline",model.ui.value.message.orEmpty().contains("Pictures/NxWallhaven"))
            val resolver=app.contentResolver
            val cursor=resolver.query(MediaStore.Images.Media.EXTERNAL_CONTENT_URI,arrayOf(MediaStore.Images.Media._ID),"${MediaStore.Images.Media.DISPLAY_NAME} LIKE ?",arrayOf("wallhaven-w5mqr6%"),"${MediaStore.Images.Media.DATE_ADDED} DESC")!!
            val uri=cursor.use{check(it.moveToFirst());android.content.ContentUris.withAppendedId(MediaStore.Images.Media.EXTERNAL_CONTENT_URI,it.getLong(0))}
            val saved=File(root,"saved-export.jpg")
            resolver.openInputStream(uri)!!.use{input->saved.outputStream().use{input.copyTo(it)}}
            assertArrayEquals(expected,digest(saved))
            repository.clearCache()
            assertTrue(saved.isFile)
            assertTrue(original.isFile)
            resolver.delete(uri,null,null)
            assertEquals(0,repository.cache().files)
        } finally {shell("svc wifi enable");shell("svc data enable")}
    }
}
