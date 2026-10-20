package io.github.nuxoul.nxwallhaven

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import io.github.nuxoul.nxwallhaven.shared.Wall
import io.github.nuxoul.nxwallhaven.ui.NxApp
import java.io.File
import kotlinx.coroutines.launch

class MainActivity:ComponentActivity() {
    private val vm:NxViewModel by viewModels()
    private var export:Pair<Wall,File>?=null
    private var localExport:io.github.nuxoul.nxwallhaven.shared.DownloadItem?=null
    private val requestNotifications=registerForActivityResult(ActivityResultContracts.RequestPermission()){ }
    private val createLocalDocument=registerForActivityResult(ActivityResultContracts.StartActivityForResult()){result ->
        val pending=localExport;localExport=null
        if(result.resultCode==RESULT_OK && pending!=null)vm.completeLocalExport(result.data?.data,pending)
    }
    private val createDocument=registerForActivityResult(ActivityResultContracts.StartActivityForResult()){result ->
        val pending=export;export=null
        if(result.resultCode==RESULT_OK && pending!=null)vm.completeExport(result.data?.data,pending.first,pending.second)
    }
    override fun onCreate(savedInstanceState:Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        savedInstanceState?.getString("export-id")?.let { id ->
            val file=savedInstanceState.getString("export-file")?.let(::File)
            if(file!=null && file.canonicalPath.startsWith(cacheDir.canonicalPath+File.separator)) {
                export=Wall(id=id,format=savedInstanceState.getString("export-mime")?:"image/jpeg") to file
            }
        }
        setContent {NxApp(vm)}
        if(intent.getBooleanExtra("open-local",false))vm.localPage()
        savedInstanceState?.getString("local-export")?.let { json ->
            localExport=runCatching { io.github.nuxoul.nxwallhaven.shared.DownloadWire.item(org.json.JSONObject(json)) }.getOrNull()
        }
        lifecycleScope.launch {repeatOnLifecycle(Lifecycle.State.STARTED){
            vm.events.collect {event ->
                when(event) {
                    is PlatformEvent.Start -> runCatching {startActivity(event.intent)}.onFailure {vm.platformFailure()}
                    is PlatformEvent.NotificationPermission -> {
                        if(android.os.Build.VERSION.SDK_INT>=33 && checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS)!=android.content.pm.PackageManager.PERMISSION_GRANTED) {
                            val flags=getSharedPreferences("notification-permission",MODE_PRIVATE)
                            if(!flags.getBoolean("asked",false)){flags.edit().putBoolean("asked",true).apply();requestNotifications.launch(android.Manifest.permission.POST_NOTIFICATIONS)}
                        }
                    }
                    is PlatformEvent.ExportLocal -> {
                        localExport=event.item
                        createLocalDocument.launch(Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
                            addCategory(Intent.CATEGORY_OPENABLE);type=event.item.wall.format;putExtra(Intent.EXTRA_TITLE,event.item.wall.filename)
                        })
                    }
                    is PlatformEvent.Export -> {
                        export=event.wall to event.file
                        createDocument.launch(Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
                            addCategory(Intent.CATEGORY_OPENABLE);type=event.wall.format
                            putExtra(Intent.EXTRA_TITLE,event.wall.filename)
                        })
                    }
                }
            }
        }}
    }
    override fun onNewIntent(intent:Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        if(intent.getBooleanExtra("open-local",false))vm.localPage()
    }
    override fun onStop() {
        android.webkit.CookieManager.getInstance().flush()
        super.onStop()
    }
    override fun onSaveInstanceState(outState:Bundle) {
        localExport?.let {item ->
            outState.putString("local-export",org.json.JSONObject().put("id",item.id).put("wall",io.github.nuxoul.nxwallhaven.shared.DownloadWire.wall(item.wall)).put("state",item.state).put("uri",item.uri).toString())
        }
        export?.let {(wall,file)->outState.putString("export-id",wall.id);outState.putString("export-file",file.absolutePath);outState.putString("export-mime",wall.format)}
        super.onSaveInstanceState(outState)
    }
}
