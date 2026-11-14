package com.shodhrag.android

import android.app.Activity
import android.webkit.WebView
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import kotlin.concurrent.thread

@InvokeArg
class TextArgs {
    lateinit var url: String
}

@InvokeArg
class DownloadArgs {
    lateinit var url: String
    lateinit var path: String
    var maxBytes: Long = 0
}

@InvokeArg
class PathArgs {
    lateinit var path: String
}

/**
 * Shodh's Android glue. Commands are called by the app's Rust side (never
 * granted to the window), which checks what it asks for; the window only
 * listens to this plugin's events.
 */
@TauriPlugin
class ShodhPlugin(private val activity: Activity) : Plugin(activity) {
    override fun load(webView: WebView) {
        super.load(webView)
    }

    /** Run [work] off the calling thread; its result or its error answers [invoke]. */
    private fun background(invoke: Invoke, work: () -> JSObject) {
        thread(name = "shodh-native") {
            try {
                invoke.resolve(work())
            } catch (e: Exception) {
                invoke.reject(e.message ?: e.javaClass.simpleName)
            }
        }
    }

    // ── The one-tap update ─────────────────────────────────────────

    @Command
    fun fetchText(invoke: Invoke) {
        val args = invoke.parseArgs(TextArgs::class.java)
        background(invoke) { JSObject().put("text", Updates.text(args.url)) }
    }

    @Command
    fun download(invoke: Invoke) {
        val args = invoke.parseArgs(DownloadArgs::class.java)
        background(invoke) {
            JSObject().put("bytes", Updates.download(activity, args.url, args.path, args.maxBytes))
        }
    }

    @Command
    fun installApk(invoke: Invoke) {
        val args = invoke.parseArgs(PathArgs::class.java)
        activity.runOnUiThread {
            try {
                invoke.resolve(JSObject().put("started", Updates.install(activity, args.path)))
            } catch (e: Exception) {
                invoke.reject(e.message ?: e.javaClass.simpleName)
            }
        }
    }
}
