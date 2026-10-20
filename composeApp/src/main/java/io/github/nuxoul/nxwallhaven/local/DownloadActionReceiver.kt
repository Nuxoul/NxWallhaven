package io.github.nuxoul.nxwallhaven.local

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import io.github.nuxoul.nxwallhaven.NxApplication
import kotlinx.coroutines.*

class DownloadActionReceiver:BroadcastReceiver() {
    override fun onReceive(context:Context,intent:Intent) {
        val id=intent.getStringExtra("id")?:return
        val state=intent.getStringExtra("state")?:return
        if(!id.matches(Regex("[A-Za-z0-9]{6}")) || state !in setOf("paused","cancelled"))return
        val pending=goAsync()
        CoroutineScope(SupervisorJob()+Dispatchers.IO).launch {
            try{runCatching{(context.applicationContext as NxApplication).downloads.setState(id,state)}}finally{pending.finish()}
        }
    }
}
