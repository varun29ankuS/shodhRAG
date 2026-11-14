package com.shodhrag.android

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import kotlin.concurrent.thread

/**
 * What arrives from outside the app: Android's share sheet ("Send to Shodh":
 * files, or text and links for "Ask about this"). Files are copied into the
 * app's own storage here, before their read permission ends; then Shodh opens
 * and takes what arrived ([Incoming.take]). Nothing is sent anywhere until the
 * user chooses where it goes.
 */
class IncomingActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val received = intent
        thread(name = "shodh-incoming") {
            val payload = try {
                read(received)
            } catch (e: Exception) {
                JSObject().put("kind", "error").put("message", e.message ?: "What was shared could not be read.")
            }
            runOnUiThread {
                Incoming.deliver(payload)
                packageManager.getLaunchIntentForPackage(packageName)?.let { launch ->
                    launch.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
                    startActivity(launch)
                }
                finish()
            }
        }
    }

    @Suppress("DEPRECATION")
    private fun streams(intent: Intent): List<Uri> = when (intent.action) {
        Intent.ACTION_SEND -> {
            val one = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                intent.getParcelableExtra(Intent.EXTRA_STREAM)
            }
            listOfNotNull(one)
        }
        Intent.ACTION_SEND_MULTIPLE -> {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
            } else {
                intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM).orEmpty()
            }
        }
        else -> emptyList()
    }

    private fun read(intent: Intent): JSObject {
        if (intent.action == android.provider.CalendarContract.ACTION_HANDLE_CUSTOM_EVENT) {
            val uri = intent.getStringExtra(android.provider.CalendarContract.EXTRA_CUSTOM_APP_URI).orEmpty()
            val id = uri.removePrefix(EVENT_LINK)
            if (uri.startsWith(EVENT_LINK) && id.isNotEmpty() && id.all { it.isLetterOrDigit() || it == '-' }) {
                return JSObject().put("kind", "event").put("id", id)
            }
            return JSObject().put("kind", "error").put("message", "This event is not one of Shodh's.")
        }
        val uris = streams(intent).take(MAX_FILES)
        if (uris.isNotEmpty()) {
            val files = JSArray()
            for (uri in uris) {
                val (file, name) = Files.copy(this, uri)
                files.put(JSObject().put("path", file.path).put("name", name))
            }
            return JSObject().put("kind", "files").put("files", files)
        }
        val text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()?.trim().orEmpty()
        if (text.isNotEmpty()) {
            val subject = intent.getStringExtra(Intent.EXTRA_SUBJECT)?.trim()
            return JSObject().put("kind", "text").put("text", text.take(MAX_TEXT)).put("subject", subject)
        }
        return JSObject().put("kind", "error").put("message", "Nothing Shodh can read was shared.")
    }

    companion object {
        const val MAX_FILES = 20
        const val MAX_TEXT = 20_000
        /** The "Open in Shodh" link of an event Shodh put in the phone's calendar. */
        const val EVENT_LINK = "shodh://event/"
    }
}

/** What arrived and is waiting for the app to take it. */
internal object Incoming {
    @Volatile
    private var pending: JSObject? = null

    @Volatile
    var listener: (() -> Unit)? = null

    fun deliver(payload: JSObject) {
        pending = payload
        listener?.invoke()
    }

    /** What arrived, once. */
    @Synchronized
    fun take(): JSObject? {
        val payload = pending
        pending = null
        return payload
    }
}
