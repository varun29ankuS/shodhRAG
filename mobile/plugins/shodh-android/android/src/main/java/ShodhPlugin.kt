package com.shodhrag.android

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.webkit.WebView
import androidx.activity.result.ActivityResult
import androidx.core.content.FileProvider
import app.tauri.PermissionState
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
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

/** The app's own FileProvider (a subclass, so it never clashes with another library's). */
class ShodhFileProvider : FileProvider()

/**
 * Shodh's Android glue. Commands are called by the app's Rust side (never
 * granted to the window), which checks what it asks for; the window only
 * listens to this plugin's events.
 */
@TauriPlugin(
    permissions = [
        Permission(strings = [Manifest.permission.CAMERA], alias = "camera"),
    ],
)
class ShodhPlugin(private val activity: Activity) : Plugin(activity) {
    /** The photo being taken, until the camera answers. */
    private var photo: File? = null

    override fun load(webView: WebView) {
        super.load(webView)
        Incoming.listener = { trigger("incoming", JSObject()) }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        trigger("incoming", JSObject())
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

    // ── Files: Add, the camera, the share sheet ────────────────────

    @Command
    fun pickFiles(invoke: Invoke) {
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT)
            .addCategory(Intent.CATEGORY_OPENABLE)
            .setType("*/*")
            .putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
        activity.runOnUiThread { startActivityForResult(invoke, intent, "filesPicked") }
    }

    @ActivityCallback
    private fun filesPicked(invoke: Invoke, result: ActivityResult) {
        val data = result.data
        val uris = buildList {
            if (result.resultCode == Activity.RESULT_OK && data != null) {
                val clip = data.clipData
                if (clip != null) {
                    for (i in 0 until clip.itemCount) add(clip.getItemAt(i).uri)
                } else {
                    data.data?.let { add(it) }
                }
            }
        }.take(IncomingActivity.MAX_FILES)
        background(invoke) {
            val files = JSArray()
            for (uri in uris) {
                val (file, name) = Files.copy(activity, uri)
                files.put(JSObject().put("path", file.path).put("name", name))
            }
            JSObject().put("files", files)
        }
    }

    @Command
    fun takePhoto(invoke: Invoke) {
        if (getPermissionState("camera") != PermissionState.GRANTED) {
            requestPermissionForAlias("camera", invoke, "cameraAnswered")
            return
        }
        openCamera(invoke)
    }

    @PermissionCallback
    private fun cameraAnswered(invoke: Invoke) {
        if (getPermissionState("camera") == PermissionState.GRANTED) {
            openCamera(invoke)
        } else {
            invoke.reject("Shodh needs the camera to take a photo. Allow it in Android's settings for Shodh.")
        }
    }

    private fun openCamera(invoke: Invoke) {
        val stamp = SimpleDateFormat("yyyy-MM-dd HH.mm.ss", Locale.US).format(Date())
        val file = File(Files.newFolder(activity), "Photo $stamp.jpg")
        photo = file
        val uri = FileProvider.getUriForFile(activity, "${activity.packageName}.shodh.files", file)
        val intent = Intent(android.provider.MediaStore.ACTION_IMAGE_CAPTURE)
            .putExtra(android.provider.MediaStore.EXTRA_OUTPUT, uri)
            .addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION or Intent.FLAG_GRANT_READ_URI_PERMISSION)
        activity.runOnUiThread { startActivityForResult(invoke, intent, "photoTaken") }
    }

    @ActivityCallback
    private fun photoTaken(invoke: Invoke, result: ActivityResult) {
        val file = photo
        photo = null
        if (file == null || result.resultCode != Activity.RESULT_OK || !file.isFile || file.length() == 0L) {
            file?.parentFile?.deleteRecursively()
            invoke.resolve(JSObject())
            return
        }
        invoke.resolve(JSObject().put("path", file.path).put("name", file.name))
    }

    @Command
    fun takeIncoming(invoke: Invoke) {
        invoke.resolve(Incoming.take() ?: JSObject().put("kind", "none"))
    }
}
