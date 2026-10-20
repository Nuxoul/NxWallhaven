package io.github.nuxoul.nxwallhaven

import android.content.Context
import android.net.Uri
import android.os.Process
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.nuxoul.nxwallhaven.local.LocalFiles
import io.github.nuxoul.nxwallhaven.shared.Wall
import java.security.MessageDigest
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

/** Two separate instrumentation processes, driven by the host with force-stop and network disabled. */
@RunWith(AndroidJUnit4::class)
class DownloadRestartProbe {
    private val app get()=InstrumentationRegistry.getInstrumentation().targetContext.applicationContext as NxApplication
    private val prefs get()=app.getSharedPreferences("alpha03-restart-probe",Context.MODE_PRIVATE)
    private fun guard(){org.junit.Assume.assumeTrue(InstrumentationRegistry.getArguments().getString("restartProbe")=="true" && (android.os.Build.FINGERPRINT.contains("generic") || android.os.Build.MODEL.contains("sdk")))}
    private fun sha(uri:String):String=app.downloads.files.input(Uri.parse(uri)).use {input->val hash=MessageDigest.getInstance("SHA-256");val b=ByteArray(65536);while(true){val n=input.read(b);if(n<0)break;hash.update(b,0,n)};hash.digest().joinToString(""){"%02x".format(it)}}
    @Test fun prepare()=runBlocking {
        guard();app.downloads.initialize()
        check(app.downloads.store.tasks().none{it.id==ID})
        val source=app.repository.image(DownloadLibraryTest.URL)
        val wall=Wall(ID,original=DownloadLibraryTest.URL,width=6240,height=4160,resolution="6240x4160")
        app.downloads.store.enqueue(wall)
        assertNotNull(app.downloads.store.acquire(ID){_,_->true})
        assertEquals("running",app.downloads.store.tasks().first{it.id==ID}.state)
        val uri=LocalFiles(app).export(wall,source,{true})
        // Intentionally omit store.complete and files.finish to model process death at this exact seam.
        check(prefs.edit().putString("uri",uri).putString("sha",sha(uri)).putInt("pid",Process.myPid()).commit())
    }
    @Test fun restoreOfflineAndReconcile()=runBlocking {
        guard()
        assertNotEquals("probe did not cross process boundary",prefs.getInt("pid",0),Process.myPid())
        app.downloads.initialize()
        val end=System.currentTimeMillis()+90_000
        var item=app.downloads.store.tasks().first{it.id==ID}
        while(item.state !in setOf("completed","failed") && System.currentTimeMillis()<end){Thread.sleep(100);item=app.downloads.store.tasks().first{it.id==ID}}
        assertEquals("completed",item.state)
        assertEquals(prefs.getString("uri",null),item.uri)
        assertEquals(prefs.getString("sha",null),sha(item.uri!!))
        assertEquals(1,app.downloads.store.library().count{it.id==ID})
        val resolver=app.contentResolver
        resolver.query(android.provider.MediaStore.Images.Media.EXTERNAL_CONTENT_URI,arrayOf(android.provider.MediaStore.Images.Media._ID),"${android.provider.MediaStore.Images.Media.DISPLAY_NAME} LIKE ?",arrayOf("wallhaven-$ID%"),null)!!.use{assertEquals("duplicate published original",1,it.count)}
        app.downloads.files.delete(item.uri!!);app.downloads.store.remove(ID,true)
        check(prefs.edit().clear().commit())
    }
    companion object {const val ID="qa0010"}
}
