package io.github.nuxoul.nxwallhaven.local

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.work.*
import io.github.nuxoul.nxwallhaven.MainActivity
import io.github.nuxoul.nxwallhaven.NxApplication
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

class DownloadWorker(context:Context,parameters:WorkerParameters):CoroutineWorker(context,parameters) {
    private val app=context.applicationContext as NxApplication
    override suspend fun doWork():Result=withContext(Dispatchers.IO) {
        val downloads=app.downloads
        downloads.initialize()
        while(!isStopped) {
            val item=downloads.store.tasks().firstOrNull{it.state=="queued"}?:return@withContext Result.success()
            try {
                if(!downloads.allowed(item.wall)){downloads.store.state(item.id,"paused");continue}
                setForeground(notification(item.id,0,null))
                val file=downloads.store.acquire(item.id){received,total ->
                    if(!isStopped)runCatching{setForegroundAsync(notification(item.id,received,total))}
                    !isStopped && downloads.allowed(item.wall) && downloads.store.tasks().find{it.id==item.id}?.state in setOf("queued","running")
                }
                val fresh=downloads.store.tasks().find{it.id==item.id}
                if(file==null || fresh?.state !in setOf("queued","running"))continue
                if(!downloads.allowed(item.wall)){downloads.store.state(item.id,"paused");continue}
                downloads.files.export(item.wall,file,keepGoing={
                    !isStopped && downloads.allowed(item.wall) && downloads.store.tasks().find{it.id==item.id}?.state in setOf("queued","running")
                },commit={uri,publish -> synchronized(downloads.publicationLock) {
                    check(!isStopped && downloads.allowed(item.wall) && downloads.store.tasks().find{it.id==item.id}?.state=="running")
                    publish()
                    downloads.store.complete(item.id,uri)
                }})
                downloads.files.finish(item.id)
            }catch(t:Exception){
                val fresh=runCatching{downloads.store.tasks().find{it.id==item.id}}.getOrNull()
                if(fresh?.state in setOf("queued","running")) {
                    if(isStopped || t is CancellationException)downloads.store.state(item.id,"queued")else downloads.store.fail(item.id)
                }
                if(t is CancellationException)throw t
                // Failure is local to this task. User retry reconciles a pending export, without duplicating it.
            }
        }
        Result.success()
    }
    private fun notification(id:String,received:Long,total:Long?):ForegroundInfo {
        val manager=applicationContext.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel(CHANNEL,"原图下载",NotificationManager.IMPORTANCE_LOW))
        val intent=Intent(applicationContext,MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP).putExtra("open-local",true)
        val pending=PendingIntent.getActivity(applicationContext,0,intent,PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val percent=total?.takeIf{it>0}?.let{(received.toDouble()*100/it).toInt().coerceIn(0,100)}
        val text=if(percent!=null)"$percent% · ${received/1024} / ${total!!/1024} KiB"else"${received/1024} KiB · 正在获取原图"
        fun control(state:String)=PendingIntent.getBroadcast(applicationContext,(id+state).hashCode(),
            Intent(applicationContext,DownloadActionReceiver::class.java).setAction("${applicationContext.packageName}.DOWNLOAD_$state").putExtra("id",id).putExtra("state",state),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val notification=NotificationCompat.Builder(applicationContext,CHANNEL)
            .setSmallIcon(android.R.drawable.stat_sys_download).setContentTitle("NxWallhaven · $id")
            .setContentText(text).setOngoing(true).setOnlyAlertOnce(true).setContentIntent(pending)
            .setProgress(100,percent?:0,percent==null)
            .addAction(0,"暂停",control("paused")).addAction(0,"取消",control("cancelled"))
            .build()
        return if(Build.VERSION.SDK_INT>=29)ForegroundInfo(301,notification,ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)else ForegroundInfo(301,notification)
    }
    companion object {
        const val CHANNEL="nxwallhaven-downloads"
        fun schedule(context:Context) {
            val work=OneTimeWorkRequestBuilder<DownloadWorker>().setBackoffCriteria(BackoffPolicy.EXPONENTIAL,30,TimeUnit.SECONDS).build()
            WorkManager.getInstance(context).enqueueUniqueWork("nxwh-download-queue",ExistingWorkPolicy.APPEND_OR_REPLACE,work)
        }
    }
}
