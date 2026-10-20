package io.github.nuxoul.nxwallhaven

import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModelStore
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.ViewModel
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.github.nuxoul.nxwallhaven.shared.Filters
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

/** Fresh model + copied safe SavedStateHandle (not Activity.recreate's retained model). Emulator only. */
@RunWith(AndroidJUnit4::class)
class RestorationAndKeyTest {
    private val instrumentation get()=InstrumentationRegistry.getInstrumentation()
    private val app get()=instrumentation.targetContext.applicationContext as NxApplication
    private fun main(block:()->Unit)=instrumentation.runOnMainSync(block)
    private fun model(handle:SavedStateHandle,store:ViewModelStore):NxViewModel {
        lateinit var result:NxViewModel
        main { result=ViewModelProvider(store,object:ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST") override fun <T:ViewModel> create(modelClass:Class<T>):T=NxViewModel(app,handle) as T
        })[NxViewModel::class.java] }
        return result
    }
    private fun waitFor(check:()->Boolean) {
        val end=System.currentTimeMillis()+15_000
        while(!check() && System.currentTimeMillis()<end)Thread.sleep(50)
        assertTrue(check())
    }
    @Before fun emulatorOnly() {
        assumeTrue("No device/account modifications",android.os.Build.FINGERPRINT.contains("generic") || android.os.Build.MODEL.contains("sdk"))
    }
    @Test fun freshViewModelRestoresFullFiltersSafeRouteAndGrid() {
        val filters=Filters("landscape","101","110","toplist","asc","3840x2160","16x9","abcdef","3M")
        val handle=SavedStateHandle(mapOf("filters-v1" to filters.savedJson(),
            "navigation-v1" to """{"version":1,"route":"settings","history":[]}""",
            "browse-grid-v1" to intArrayOf(8,37)))
        val firstStore=ViewModelStore();val secondStore=ViewModelStore()
        try {
            val first=model(handle,firstStore)
            assertEquals(filters,first.ui.value.filters)
            assertEquals("settings",first.ui.value.route)
            assertFalse("settings startup must not fetch browse pages",first.ui.value.loading)
            assertEquals(8 to 37,first.gridPosition(false))
            val snapshot=HashMap<String,Any?>()
            main { for(key in handle.keys())snapshot[key]=handle.get<Any>(key);firstStore.clear() }
            val second=model(SavedStateHandle(snapshot),secondStore)
            assertNotSame(first,second)
            assertEquals(filters,second.ui.value.filters)
            assertEquals("settings",second.ui.value.route)
            assertEquals(8 to 37,second.gridPosition(false))
            assertFalse(snapshot.keys.any { it.contains("cookie",true) || it.contains("cipher",true) })
            val nav=JSONObject(snapshot["navigation-v1"] as String)
            assertFalse(nav.has("file"));assertFalse(nav.has("key"))
        } finally {main { firstStore.clear();secondStore.clear() }}
    }
    @Test fun malformedRoutesAndFiltersCannotRestorePermissionOrCredentials() {
        val store=ViewModelStore()
        try {
            val handle=SavedStateHandle(mapOf("filters-v1" to """{"version":1,"apikey":"fixture"}""",
                "navigation-v1" to """{"version":1,"route":"untrusted","detail":"../../bad","username":"../bad"}"""))
            val vm=model(handle,store)
            assertEquals(Filters(),vm.ui.value.filters)
            assertEquals("browse",vm.ui.value.route)
            assertNull(vm.ui.value.detail)
            assertNull(vm.ui.value.profile)
        } finally { main {store.clear()} }
    }
    @Test fun keyPresenceAndLocalRejectionNeverClaimServerValidity() {
        val store=ViewModelStore()
        try {
            val vm=model(SavedStateHandle(mapOf("navigation-v1" to """{"version":1,"route":"settings"}""")),store)
            assertFalse(vm.ui.value.keyStatus.contains("已确认"))
            // Invalid header is rejected offline by the real UniFFI bridge, not sent or saved.
            main { vm.verifyKey("invalid\r\nheader") }
            waitFor { !vm.ui.value.keyChecking }
            assertFalse(vm.ui.value.keyStatus.contains("已确认"))
            main {vm.keyInputChanged()}
            assertTrue(vm.ui.value.keyStatus.contains("尚未验证"))
        } finally {main {store.clear()} }
    }
    @Test fun freshDetailRestorationInitializesPreviewStateBeforeReload() {
        val store=ViewModelStore()
        try {
            val handle=SavedStateHandle(mapOf("navigation-v1" to """{"version":1,"route":"detail","detail":"abc123","history":[{"route":"browse"}]}"""))
            val vm=model(handle,store)
            assertEquals("detail",vm.ui.value.route)
            assertEquals("abc123",vm.ui.value.detail?.id)
            assertEquals("browse",vm.detailBaseRoute())
        }finally{main{store.clear()}}
    }
}
