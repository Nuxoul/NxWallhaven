package io.github.nuxoul.nxwallhaven

import androidx.activity.compose.setContent
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.ViewModelProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.UiDevice
import io.github.nuxoul.nxwallhaven.local.*
import io.github.nuxoul.nxwallhaven.shared.*
import java.io.File
import java.io.InputStream
import java.security.MessageDigest
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Before
import org.junit.After
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** All mutations and network switches are confined to the dedicated disposable emulator. */
@RunWith(AndroidJUnit4::class)
class DownloadLibraryTest {
    private var prepared=false
    @get:Rule val compose=createAndroidComposeRule<MainActivity>()
    private val app get()=compose.activity.application as NxApplication
    private lateinit var vm:NxViewModel
    private val ids=listOf("qa0001","qa0002","qa0003","qa0004")
    private fun shell(command:String){InstrumentationRegistry.getInstrumentation().uiAutomation.executeShellCommand(command).use{java.io.FileInputStream(it.fileDescriptor).use{input->input.readBytes()}}}
    private fun waitFor(condition:()->Boolean)=compose.waitUntil(90_000){condition()}
    private fun wall(id:String)=Wall(id,original=URL,width=6240,height=4160,resolution="6240x4160",tags=listOf(7 to "fixture-landscape"),uploader="LocalFixture")
    private fun digest(input:InputStream)=input.use{val hash=MessageDigest.getInstance("SHA-256");val buffer=ByteArray(65536);while(true){val n=it.read(buffer);if(n<0)break;hash.update(buffer,0,n)};hash.digest()}
    private fun task(id:String)=app.downloads.store.tasks().first{it.id==id}
    private fun clean(){for(id in ids){runCatching{val item=app.downloads.store.tasks().find{it.id==id};if(item?.active==true)app.downloads.store.state(id,"cancelled");item?.uri?.takeIf{it.startsWith("content://media/")}?.let{app.downloads.files.delete(it)};app.downloads.files.discardPending(id);app.downloads.store.remove(id,true)}}}
    @Before fun prepare()=runBlocking {
        org.junit.Assume.assumeTrue(android.os.Build.FINGERPRINT.contains("generic") || android.os.Build.MODEL.contains("sdk"))
        prepared=true
        if(android.os.Build.VERSION.SDK_INT>=33)shell("cmd appops set ${app.packageName} POST_NOTIFICATION deny")
        if(android.os.Build.VERSION.SDK_INT>=33)assertFalse(app.getSystemService(android.app.NotificationManager::class.java).areNotificationsEnabled())
        shell("svc wifi enable");shell("svc data enable")
        app.getSharedPreferences("notification-permission",Context.MODE_PRIVATE).edit().putBoolean("asked",true).commit()
        Preferences(app).clearKey();app.downloads.initialize();clean()
        compose.runOnUiThread{vm=ViewModelProvider(compose.activity)[NxViewModel::class.java]}
    }
    @After fun cleanup(){if(!prepared)return;shell("svc wifi enable");shell("svc data enable");shell("cmd appops set ${app.packageName} POST_NOTIFICATION default");clean()}

    @Test fun offlineWorkerDedupLocalViewerAndConfirmedDeletion()=runBlocking {
        val cached=app.repository.image(URL);val expected=digest(cached.inputStream())
        val unchanged=Preferences(app).snapshot()
        shell("svc wifi disable");shell("svc data disable")
        // Real WorkManager execution while the activity is stopped, not a direct worker call.
        compose.activityRule.scenario.moveToState(Lifecycle.State.CREATED)
        assertEquals(1,app.downloads.enqueue(listOf(wall("qa0001"),wall("qa0001"))))
        waitFor{app.downloads.store.tasks().any{it.id=="qa0001" && it.state in setOf("completed","failed")}}
        assertEquals("completed",task("qa0001").state)
        val uri=task("qa0001").uri!!
        assertArrayEquals(expected,digest(app.downloads.files.input(Uri.parse(uri))))
        assertEquals(0,app.downloads.enqueue(listOf(wall("qa0001"))))
        assertEquals(uri,task("qa0001").uri)
        assertEquals(1,app.downloads.store.library().count{it.id=="qa0001"})
        compose.activityRule.scenario.moveToState(Lifecycle.State.RESUMED)
        compose.runOnUiThread{vm.localPage();vm.local.tab(1)}
        waitFor{vm.local.ui.value.library.any{it.id=="qa0001"}}
        compose.onNodeWithTag("library-card-qa0001").performClick()
        waitFor{vm.local.ui.value.viewer!=null}
        compose.onNodeWithTag("local-original").assertIsDisplayed()
        compose.waitUntil(60_000){compose.onAllNodesWithText("完整原图 ·",substring=true).fetchSemanticsNodes().isNotEmpty()}
        UiDevice.getInstance(InstrumentationRegistry.getInstrumentation()).takeScreenshot(File(app.getExternalFilesDir(null),"alpha03-local-original.png"))
        compose.onNodeWithTag("local-back").performClick()
        compose.runOnUiThread{vm.local.filter("no-match",0,"all",0)}
        assertTrue(vm.local.ui.value.library.isEmpty())
        compose.runOnUiThread{vm.local.filter("fixture-landscape",2,"sfw",6000)}
        assertEquals(listOf("qa0001"),vm.local.ui.value.library.map{it.id})
        compose.runOnUiThread{vm.local.select("qa0001")}
        compose.onNodeWithTag("delete-local-selected").performClick()
        compose.onNodeWithTag("confirm-remove-local-records").performClick()
        waitFor{!vm.local.ui.value.busy && app.downloads.store.library().none{it.id=="qa0001"}}
        assertTrue(app.downloads.files.exists(uri))
        // Re-index only the same fixture-owned URI to exercise actual confirmed file deletion.
        app.downloads.store.enqueue(wall("qa0001"));app.downloads.store.complete("qa0001",uri)
        compose.runOnUiThread{vm.local.refresh()};waitFor{vm.local.ui.value.library.any{it.id=="qa0001"}}
        compose.runOnUiThread{vm.local.select("qa0001")}
        compose.onNodeWithTag("delete-local-selected").performClick()
        compose.onNodeWithTag("confirm-delete-local-files").performClick()
        waitFor{!vm.local.ui.value.busy && app.downloads.store.library().none{it.id=="qa0001"}}
        assertEquals(LocalFiles.Availability.MISSING,app.downloads.files.availability(uri))
        assertTrue(cached.isFile);assertArrayEquals(expected,digest(cached.inputStream()))
        assertEquals(unchanged,Preferences(app).snapshot())
    }

    @Test fun notificationActionsPersistAndPermissionRevocationPauses()=runBlocking {
        app.repository.image(URL)
        app.downloads.store.enqueue(wall("qa0002"))
        for(state in listOf("paused","cancelled")) {
            if(state=="cancelled")app.downloads.store.state("qa0002","queued")
            val reached=java.util.concurrent.CountDownLatch(1)
            val release=java.util.concurrent.CountDownLatch(1)
            val executor=java.util.concurrent.Executors.newSingleThreadExecutor()
            val future=executor.submit<File?>{app.downloads.store.acquire("qa0002"){_,_->reached.countDown();check(release.await(30,java.util.concurrent.TimeUnit.SECONDS));true}}
            try {
                assertTrue(reached.await(30,java.util.concurrent.TimeUnit.SECONDS))
                assertEquals("running",task("qa0002").state)
                app.sendBroadcast(Intent(app,DownloadActionReceiver::class.java).putExtra("id","qa0002").putExtra("state",state))
                waitFor{task("qa0002").state==state}
                release.countDown()
                assertNull(future.get(30,java.util.concurrent.TimeUnit.SECONDS))
                assertTrue(app.downloads.store.library().none{it.id=="qa0002"})
            }finally{release.countDown();executor.shutdownNow()}
        }
        val reopened=DownloadStore(app.cacheDir,app.filesDir)
        assertEquals("cancelled",reopened.tasks().first{it.id=="qa0002"}.state)
        reopened.enqueue(wall("qa0002"));reopened.state("qa0002","paused")
        compose.runOnUiThread{vm.localPage();vm.local.tab(0)}
        waitFor{vm.local.ui.value.tasks.any{it.id=="qa0002" && it.state=="paused"}}
        compose.onNodeWithTag("queue-resume-qa0002").performClick()
        waitFor{task("qa0002").state in setOf("completed","failed")}
        assertEquals("completed",task("qa0002").state)
        val restricted=wall("qa0003").copy(purity="nsfw")
        app.downloads.store.enqueue(restricted)
        app.downloads.permissionsChanged(true)
        waitFor{task("qa0003").state=="paused"}
        assertFalse(app.downloads.allowed(restricted))
        assertTrue(app.downloads.allowed(wall("qa0002")))
    }

    @Test fun inaccessibleIndexIsRetainedAndPublishedExportReconciles()=runBlocking {
        app.downloads.store.enqueue(wall("qa0004"));app.downloads.store.complete("qa0004","content://denied.fixture/image/1")
        compose.runOnUiThread{vm.localPage();vm.local.tab(1)}
        waitFor{vm.local.ui.value.library.any{it.id=="qa0004"}}
        compose.runOnUiThread{vm.local.select("qa0004");vm.local.deleteSelected(true)}
        waitFor{!vm.local.ui.value.busy}
        assertEquals("completed",task("qa0004").state)
        assertTrue(app.downloads.store.library().any{it.id=="qa0004"})
        assertTrue(vm.local.ui.value.message.orEmpty().contains("无删除权限"))
        val source=app.repository.image(URL)
        val files=LocalFiles(app)
        val saved=files.export(wall("qa0002"),source,{true})
        // Simulates a crash after publication, before SQLite completion/journal removal.
        val reconciled=LocalFiles(app).export(wall("qa0002"),source,{true})
        assertEquals(saved,reconciled)
        assertArrayEquals(digest(source.inputStream()),digest(files.input(Uri.parse(saved))))
        app.downloads.store.enqueue(wall("qa0002"));app.downloads.store.complete("qa0002",saved);files.finish("qa0002")
    }
    companion object {const val URL="https://w.wallhaven.cc/full/w5/wallhaven-w5mqr6.jpg"}
    @Test fun batchDownloadRequiresExplicitConfirmation()=runBlocking {
        val source=app.repository.image(URL)
        compose.runOnUiThread {compose.activity.setContent {
            androidx.compose.material3.MaterialTheme {io.github.nuxoul.nxwallhaven.ui.BatchDownloadButton(vm,listOf(wall("qa0001")))}
        }}
        compose.onNodeWithTag("batch-download").performClick()
        assertTrue(app.downloads.store.tasks().none{it.id=="qa0001"})
        compose.onNodeWithText("取消").performClick()
        assertTrue(app.downloads.store.tasks().none{it.id=="qa0001"})
        compose.onNodeWithTag("batch-download").performClick()
        compose.onNodeWithTag("confirm-batch-download").performClick()
        waitFor{app.downloads.store.tasks().any{it.id=="qa0001" && it.state in setOf("completed","failed")}}
        assertEquals("completed",task("qa0001").state)
        assertArrayEquals(digest(source.inputStream()),digest(app.downloads.files.input(Uri.parse(task("qa0001").uri!!))))
    }
}
