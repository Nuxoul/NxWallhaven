package io.github.nuxoul.nxwallhaven

import android.app.Application
import io.github.nuxoul.nxwallhaven.shared.Repository

class NxApplication : Application() {
    companion object {
        init { System.loadLibrary("nxwallhaven_core") }
        @JvmStatic private external fun initTls(context: android.content.Context)
    }
    override fun onCreate() { super.onCreate(); initTls(this) }
    val repository by lazy { Repository(cacheDir) }
    val webSession by lazy { io.github.nuxoul.nxwallhaven.account.WebSession(this) }
}
