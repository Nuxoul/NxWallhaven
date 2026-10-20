package io.github.nuxoul.nxwallhaven

import android.graphics.Bitmap
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.staggeredgrid.rememberLazyStaggeredGridState
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.lifecycle.ViewModelProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.nuxoul.nxwallhaven.shared.*
import io.github.nuxoul.nxwallhaven.ui.*
import java.io.File
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class MobileBrowsingExperienceTest {
    @get:Rule val compose=createAndroidComposeRule<MainActivity>()
    private fun guard(){org.junit.Assume.assumeTrue(android.os.Build.MODEL.contains("sdk") || android.os.Build.FINGERPRINT.contains("generic"))}
    @Test fun previewStaysVisibleWhileOriginalBytesAdvance() {
        guard();val context=InstrumentationRegistry.getInstrumentation().targetContext
        val file=File(context.cacheDir,"experience-preview.png")
        val bitmap=Bitmap.createBitmap(32,24,Bitmap.Config.ARGB_8888)
        try {
            bitmap.eraseColor(android.graphics.Color.MAGENTA);file.outputStream().use{check(bitmap.compress(Bitmap.CompressFormat.PNG,100,it))};bitmap.recycle()
            val ready=mutableStateOf(false)
            compose.activity.setContent {MaterialTheme {OriginalPreview(file,ready.value,TransferProgress(1024,4096),"正在获取原图",Modifier.fillMaxSize())}}
            compose.waitUntil(10_000){compose.onAllNodesWithTag("detail-thumbnail-loaded").fetchSemanticsNodes().isNotEmpty()}
            compose.onNodeWithTag("detail-thumbnail-loaded").assertIsDisplayed()
            compose.onNodeWithTag("original-progress-bar").assertIsDisplayed()
            compose.onNodeWithText("已接收",substring=true).assertIsDisplayed()
            compose.runOnUiThread{ready.value=true}
            compose.onNodeWithTag("detail-thumbnail-loaded").assertDoesNotExist()
            compose.onNodeWithTag("original-progress-bar").assertDoesNotExist()
        }finally{file.delete();if(!bitmap.isRecycled)bitmap.recycle()}
    }
    @Test fun actualGalleryPreparesThreePagesBeforeAThumbSwipe() {
        guard();lateinit var vm:NxViewModel
        compose.runOnUiThread{vm=ViewModelProvider(compose.activity)[NxViewModel::class.java]}
        val all=(0 until 120).map{Wall("t${it.toString().padStart(5,'0')}",width=100,height=100)}
        val count=mutableIntStateOf(24);val calls=java.util.concurrent.atomic.AtomicInteger()
        compose.activity.setContent {MaterialTheme {Gallery(vm,all.take(count.intValue),false,false,null,1,false,rememberLazyStaggeredGridState(),true,
            onMore={calls.incrementAndGet();count.intValue=(count.intValue+24).coerceAtMost(all.size)},onRefresh={},modifier=Modifier.fillMaxSize())}}
        compose.waitUntil(10_000){count.intValue>=72}
        assertEquals("first runway should be bounded, not crawl all pages",72,count.intValue)
        assertEquals(2,calls.get())
        compose.onNodeWithTag("wallpaper-grid").performScrollToIndex(48)
        compose.waitUntil(10_000){count.intValue>=96}
        assertTrue(calls.get() in 3..4)
    }
}
