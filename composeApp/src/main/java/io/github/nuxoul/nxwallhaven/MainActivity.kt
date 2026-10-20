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
        lifecycleScope.launch {repeatOnLifecycle(Lifecycle.State.STARTED){
            vm.events.collect {event ->
                when(event) {
                    is PlatformEvent.Start -> runCatching {startActivity(event.intent)}.onFailure {vm.platformFailure()}
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
    override fun onStop() {
        android.webkit.CookieManager.getInstance().flush()
        super.onStop()
    }
    override fun onSaveInstanceState(outState:Bundle) {
        export?.let {(wall,file)->outState.putString("export-id",wall.id);outState.putString("export-file",file.absolutePath);outState.putString("export-mime",wall.format)}
        super.onSaveInstanceState(outState)
    }
}
