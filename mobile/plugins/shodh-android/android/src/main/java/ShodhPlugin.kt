package com.shodhrag.android

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.util.Log
import android.view.View
import android.webkit.WebView
import androidx.activity.result.ActivityResult
import androidx.core.content.FileProvider
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
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

/** Where the status bar, the navigation bar and a camera cutout cover the window, in CSS pixels (dp). */
data class SystemInsets(val top: Float, val right: Float, val bottom: Float, val left: Float) {
    fun toJS(): JSObject = JSObject()
        .put("top", top.toDouble())
        .put("right", right.toDouble())
        .put("bottom", bottom.toDouble())
        .put("left", left.toDouble())
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
        Permission(strings = [Manifest.permission.READ_CALENDAR, Manifest.permission.WRITE_CALENDAR], alias = "calendar"),
        Permission(strings = [Manifest.permission.POST_NOTIFICATIONS], alias = "notifications"),
        Permission(strings = [Manifest.permission.RECORD_AUDIO], alias = "microphone"),
    ],
)
class ShodhPlugin(private val activity: Activity) : Plugin(activity) {
    /** The photo being taken, until the camera answers. */
    private var photo: File? = null

    /** Watches the phone's calendars while the app runs (once it may read them). */
    private var calendarWatch: android.database.ContentObserver? = null

    /** The app's WebView, whose window insets the page is told. */
    private var webView: WebView? = null

    /** The system bars' insets as the window last reported them (UI thread only). */
    private var insets = SystemInsets(0f, 0f, 0f, 0f)

    override fun load(webView: WebView) {
        super.load(webView)
        this.webView = webView
        Incoming.listener = { trigger("incoming", JSObject()) }
        watchCalendar()
        briefFrom(activity.intent)
        watchInsets(webView)
    }

    /** A brief's notification was tapped: the app opens at that brief. */
    private fun briefFrom(intent: Intent?) {
        val eventId = intent?.getStringExtra(Notifier.EXTRA_BRIEF) ?: return
        intent.removeExtra(Notifier.EXTRA_BRIEF)
        Incoming.deliver(JSObject().put("kind", "brief").put("eventId", eventId))
    }

    override fun onResume() {
        super.onResume()
        Notifier.foreground = true
    }

    override fun onPause() {
        super.onPause()
        Notifier.foreground = false
    }

    private fun watchCalendar() {
        if (calendarWatch != null || getPermissionState("calendar") != PermissionState.GRANTED) return
        calendarWatch = PhoneCalendar.watch(activity) { trigger("calendar", JSObject()) }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        briefFrom(intent)
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

    // ── The system bars ──────────────────────────────────────────

    /** Light icons on the dark theme, dark icons on the light one. */
    @Command
    fun setSystemBars(invoke: Invoke) {
        val dark = invoke.getArgs().getBoolean("dark", true) ?: true
        activity.runOnUiThread {
            val window = activity.window
            val bars = androidx.core.view.WindowCompat.getInsetsController(window, window.decorView)
            bars.isAppearanceLightStatusBars = !dark
            bars.isAppearanceLightNavigationBars = !dark
            invoke.resolve()
        }
    }

    /**
     * The app draws edge to edge (MainActivity's enableEdgeToEdge), under the
     * status bar and the gesture bar, and the WebView does not report those
     * bars to the page: env(safe-area-inset-*) is 0 in it. So the page is
     * told here, in CSS pixels: on each change (an "insets" event) and when
     * it asks (systemInsets). The WebView's own handling of the insets is
     * kept, so a WebView that does report them still can.
     */
    private fun watchInsets(webView: WebView) {
        activity.runOnUiThread {
            ViewCompat.setOnApplyWindowInsetsListener(webView) { view, windowInsets ->
                val next = measureInsets(view, windowInsets)
                if (next != insets) {
                    insets = next
                    Log.i(
                        "Shodh",
                        "Shodh system bars: top ${next.top} dp, right ${next.right} dp, bottom ${next.bottom} dp, left ${next.left} dp",
                    )
                    trigger("insets", next.toJS())
                }
                ViewCompat.onApplyWindowInsets(view, windowInsets)
            }
            ViewCompat.requestApplyInsets(webView)
        }
    }

    /** The status bar, the navigation bar and a cutout, from the window's own insets, in dp. */
    private fun measureInsets(view: View, dispatched: WindowInsetsCompat?): SystemInsets {
        val window = ViewCompat.getRootWindowInsets(view) ?: dispatched ?: return insets
        val bars = window.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
        val density = view.resources.displayMetrics.density
        return SystemInsets(bars.top / density, bars.right / density, bars.bottom / density, bars.left / density)
    }

    @Command
    fun systemInsets(invoke: Invoke) {
        activity.runOnUiThread {
            val now = webView?.let { measureInsets(it, null) } ?: insets
            invoke.resolve(now.toJS())
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

    // ── Voice ──────────────────────────────────────────────────────

    private val voice by lazy { Voice(activity) }

    @Command
    fun voiceListen(invoke: Invoke) {
        if (getPermissionState("microphone") != PermissionState.GRANTED) {
            requestPermissionForAlias("microphone", invoke, "microphoneAnswered")
            return
        }
        listenNow(invoke)
    }

    @PermissionCallback
    private fun microphoneAnswered(invoke: Invoke) {
        if (getPermissionState("microphone") == PermissionState.GRANTED) listenNow(invoke)
        else invoke.reject("Shodh needs the microphone to listen. Allow it in Android's settings for Shodh.")
    }

    private fun listenNow(invoke: Invoke) {
        voice.listen { text, error ->
            if (error != null) invoke.reject(error) else invoke.resolve(JSObject().put("text", text ?: ""))
        }
    }

    @Command
    fun voiceStopListening(invoke: Invoke) {
        voice.stopListening()
        invoke.resolve()
    }

    @Command
    fun voiceSpeak(invoke: Invoke) {
        val text = invoke.getArgs().getString("text", "") ?: ""
        voice.speak(text) { error -> if (error != null) invoke.reject(error) else invoke.resolve() }
    }

    @Command
    fun voiceStop(invoke: Invoke) {
        voice.stopSpeaking()
        invoke.resolve()
    }

    // ── Notifications ──────────────────────────────────────────────

    @Command
    fun notificationsAccess(invoke: Invoke) {
        if (android.os.Build.VERSION.SDK_INT < android.os.Build.VERSION_CODES.TIRAMISU ||
            getPermissionState("notifications") == PermissionState.GRANTED
        ) {
            invoke.resolve(JSObject().put("granted", androidx.core.app.NotificationManagerCompat.from(activity).areNotificationsEnabled()))
            return
        }
        requestPermissionForAlias("notifications", invoke, "notificationsAnswered")
    }

    @PermissionCallback
    private fun notificationsAnswered(invoke: Invoke) {
        invoke.resolve(JSObject().put("granted", getPermissionState("notifications") == PermissionState.GRANTED))
    }

    @Command
    fun startConnection(invoke: Invoke) {
        activity.runOnUiThread {
            try {
                ConnectionService.start(activity)
                invoke.resolve()
            } catch (e: Exception) {
                invoke.reject(e.message ?: "Shodh could not stay connected.")
            }
        }
    }

    @Command
    fun stopConnection(invoke: Invoke) {
        ConnectionService.stop(activity)
        invoke.resolve()
    }

    @Command
    fun pushDistributors(invoke: Invoke) {
        val list = JSArray()
        for (name in Push.distributors(activity)) list.put(name)
        invoke.resolve(JSObject().put("distributors", list))
    }

    @Command
    fun pushRegister(invoke: Invoke) {
        val distributor = invoke.getArgs().getString("distributor", null)
        background(invoke) {
            Push.register(activity, distributor ?: throw IllegalArgumentException("Choose a push app."))
            JSObject()
        }
    }

    @Command
    fun pushUnregister(invoke: Invoke) {
        Push.unregister(activity)
        invoke.resolve()
    }

    @Command
    fun pushTakeEndpoint(invoke: Invoke) {
        invoke.resolve(JSObject().put("endpoint", Push.takeEndpoint(activity)))
    }

    @Command
    fun scheduleBriefs(invoke: Invoke) {
        val list = invoke.getArgs().getJSONArray("briefs")
        val briefs = (0 until list.length()).associate {
            val item = list.getJSONObject(it)
            item.getString("eventId") to item.getLong("atMs")
        }
        Briefs.schedule(activity, briefs)
        invoke.resolve()
    }

    // ── The phone's own calendar ──────────────────────────────────

    @Command
    fun calendarAccess(invoke: Invoke) {
        if (getPermissionState("calendar") == PermissionState.GRANTED) {
            watchCalendar()
            invoke.resolve(JSObject().put("granted", true))
            return
        }
        requestPermissionForAlias("calendar", invoke, "calendarAnswered")
    }

    @PermissionCallback
    private fun calendarAnswered(invoke: Invoke) {
        val granted = getPermissionState("calendar") == PermissionState.GRANTED
        if (granted) watchCalendar()
        invoke.resolve(JSObject().put("granted", granted))
    }

    private fun needCalendar(invoke: Invoke): Boolean {
        if (getPermissionState("calendar") == PermissionState.GRANTED) return true
        invoke.reject("Shodh may not use this phone's calendar. Allow it in Settings, Calendar.")
        return false
    }

    @Command
    fun calendarList(invoke: Invoke) {
        if (!needCalendar(invoke)) return
        background(invoke) { JSObject().put("calendars", PhoneCalendar.list(activity)) }
    }

    @Command
    fun calendarEnsureShodh(invoke: Invoke) {
        if (!needCalendar(invoke)) return
        background(invoke) { JSObject().put("id", PhoneCalendar.ensureShodh(activity)) }
    }

    @Command
    fun calendarEvents(invoke: Invoke) {
        if (!needCalendar(invoke)) return
        val args = invoke.getArgs()
        val ids = args.getJSONArray("calendarIds")
        val calendars = (0 until ids.length()).map { ids.getLong(it) }
        background(invoke) {
            JSObject().put("events", PhoneCalendar.events(activity, calendars, args.getLong("fromMs"), args.getLong("toMs")))
        }
    }

    @Command
    fun calendarInsert(invoke: Invoke) {
        if (!needCalendar(invoke)) return
        val args = invoke.getArgs()
        val fields = args.getJSObject("fields") ?: JSObject()
        background(invoke) { JSObject().put("eventId", PhoneCalendar.insert(activity, args.getLong("calendarId"), fields)) }
    }

    @Command
    fun calendarUpdate(invoke: Invoke) {
        if (!needCalendar(invoke)) return
        val args = invoke.getArgs()
        val fields = args.getJSObject("fields") ?: JSObject()
        background(invoke) {
            PhoneCalendar.update(activity, args.getLong("calendarId"), args.getLong("eventId"), fields)
            JSObject()
        }
    }

    @Command
    fun calendarDelete(invoke: Invoke) {
        if (!needCalendar(invoke)) return
        val args = invoke.getArgs()
        background(invoke) {
            PhoneCalendar.delete(activity, args.getLong("calendarId"), args.getLong("eventId"))
            JSObject()
        }
    }
}
