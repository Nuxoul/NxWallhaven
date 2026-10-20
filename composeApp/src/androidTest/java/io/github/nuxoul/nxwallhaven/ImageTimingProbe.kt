package io.github.nuxoul.nxwallhaven

import android.os.SystemClock
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.ui.Modifier
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import io.github.nuxoul.nxwallhaven.shared.Repository
import com.davemorrissey.labs.subscaleview.ImageSource
import com.davemorrissey.labs.subscaleview.SubsamplingScaleImageView
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** Public SFW image timing, isolated cache; no credential/config/Cookie values are recorded. */
@RunWith(AndroidJUnit4::class)
class ImageTimingProbe {
    @get:Rule val compose=createAndroidComposeRule<MainActivity>()
    @Test fun publicOriginalColdWarmAndFirstTiles()=runBlocking {
        org.junit.Assume.assumeTrue(android.os.Build.MODEL.contains("sdk") || android.os.Build.FINGERPRINT.contains("generic"))
        val app=compose.activity.application as NxApplication
        val root=File(app.cacheDir,"timing-probe-${java.util.UUID.randomUUID()}").apply{mkdirs()}
        val repository=Repository(root)
        try {
            repository.ping()
            val wall=repository.detail("w5mqr6",null)
            suspend fun measure(label:String,url:String):File {
                val start=SystemClock.elapsedRealtime()
                val file=try{repository.image(url)}catch(t:Exception){println("NXWH_TIMING $label failed_ms=${SystemClock.elapsedRealtime()-start} kind=${t.javaClass.simpleName}");throw t}
                println("NXWH_TIMING $label ms=${SystemClock.elapsedRealtime()-start} bytes=${file.length()}")
                return file
            }
            measure("thumbnail-cold",checkNotNull(wall.thumbnail))
            measure("thumbnail-warm",checkNotNull(wall.thumbnail))
            val nativeStart=SystemClock.elapsedRealtime()
            val connection=java.net.URL(checkNotNull(wall.original)).openConnection() as javax.net.ssl.HttpsURLConnection
            try {
                connection.connectTimeout=10_000;connection.readTimeout=10_000;connection.instanceFollowRedirects=false
                connection.setRequestProperty("User-Agent","NxWallhaven/0.1 (image cache)")
                val status=connection.responseCode
                val headersMs=SystemClock.elapsedRealtime()-nativeStart
                var bytes=0L
                connection.inputStream.use{input->val buffer=ByteArray(65536);while(true){val n=input.read(buffer);if(n<0)break;bytes+=n}}
                println("NXWH_TIMING system-https-original status=$status headers_ms=$headersMs total_ms=${SystemClock.elapsedRealtime()-nativeStart} bytes=$bytes")
            }catch(t:Exception){println("NXWH_TIMING system-https-original failed_ms=${SystemClock.elapsedRealtime()-nativeStart} kind=${t.javaClass.simpleName}")}
            finally{connection.disconnect()}
            val original=measure("original-cold",checkNotNull(wall.original))
            measure("original-warm",checkNotNull(wall.original))
            val loaded=CountDownLatch(1)
            val failure=AtomicReference<String?>()
            val start=SystemClock.elapsedRealtime()
            compose.runOnUiThread {compose.activity.setContent {
                AndroidView(factory={context -> SubsamplingScaleImageView(context).apply {
                    setOrientation(SubsamplingScaleImageView.ORIENTATION_USE_EXIF)
                    setMinimumScaleType(SubsamplingScaleImageView.SCALE_TYPE_CENTER_INSIDE)
                    setMaxScale(8f);setMaxTileSize(2048);setMinimumTileDpi(160)
                    setOnImageEventListener(object:SubsamplingScaleImageView.DefaultOnImageEventListener(){
                        override fun onImageLoaded(){println("NXWH_TIMING original-first-tiles ms=${SystemClock.elapsedRealtime()-start} dimensions=${sWidth}x${sHeight}");loaded.countDown()}
                        override fun onImageLoadError(e:Exception){failure.set(e.javaClass.simpleName);loaded.countDown()}
                        override fun onTileLoadError(e:Exception){failure.set(e.javaClass.simpleName);loaded.countDown()}
                    })
                    setImage(ImageSource.uri(android.net.Uri.fromFile(original)).tilingEnabled())
                }},modifier=Modifier.fillMaxSize(),onRelease={it.recycle()})
            }}
            compose.waitUntil(60_000){loaded.count==0L}
            assertNull("first tile decode failed",failure.get())
        }finally{repository.clearCache();root.deleteRecursively()}
    }
}
