package io.github.nuxoul.nxwallhaven

import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.Process
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import io.github.nuxoul.nxwallhaven.shared.Repository
import java.io.File
import java.security.MessageDigest
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

/** Run these two probes separately with force-stop + network disabled between them. */
@RunWith(AndroidJUnit4::class)
class CacheRestartProbe {
    @org.junit.Before fun explicitEmulatorProbeOnly() {
        org.junit.Assume.assumeTrue(androidx.test.platform.app.InstrumentationRegistry.getArguments().getString("restartProbe")=="true")
        org.junit.Assume.assumeTrue(android.os.Build.FINGERPRINT.contains("generic") || android.os.Build.MODEL.contains("sdk"))
    }
    private val app get()=ApplicationProvider.getApplicationContext<NxApplication>()
    private val root get()=File(app.cacheDir,"restart-proof").apply{mkdirs()}
    private val url="https://w.wallhaven.cc/full/w5/wallhaven-w5mqr6.jpg"
    private fun sha(file:File):String=file.inputStream().use {stream->
        val hash=MessageDigest.getInstance("SHA-256");val buffer=ByteArray(65536)
        while(true){val n=stream.read(buffer);if(n<0)break;hash.update(buffer,0,n)}
        hash.digest().joinToString(""){"%02x".format(it)}
    }
    @Test fun prepare()=runBlocking {
        val file=Repository(root).image(url)
        File(root,"expected.sha256").writeText(sha(file))
        File(root,"first.pid").writeText(Process.myPid().toString())
        assertTrue(file.length()>1_000_000)
    }
    @Test fun offlineAfterRestart()=runBlocking {
        assertNotEquals(File(root,"first.pid").readText(),Process.myPid().toString())
        val network=app.getSystemService(ConnectivityManager::class.java)
        assertFalse("restart probe must actually be offline",network.getNetworkCapabilities(network.activeNetwork)?.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED)==true)
        val repository=Repository(root)
        val file=repository.image(url)
        assertEquals(File(root,"expected.sha256").readText(),sha(file))
        val exported=File(app.getExternalFilesDir(null),"restart-offline-original.jpg")
        file.inputStream().use{input->exported.outputStream().use{input.copyTo(it)}}
        assertEquals(sha(file),sha(exported))
        repository.clearCache()
        assertTrue(exported.isFile)
        assertTrue(exported.delete())
    }
}
