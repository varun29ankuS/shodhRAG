package com.shodhrag.android

import android.app.AlarmManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.ServiceCompat
import java.util.UUID

/**
 * "Stay connected": a foreground service that keeps Shodh running, with its
 * lasting "Shodh is connected" notification, so the link to the user's own
 * computer stays open and an approval or a finished answer can be told. It
 * runs only when the user chose Stay connected.
 */
class ConnectionService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val type = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE
        } else {
            0
        }
        ServiceCompat.startForeground(this, NOTIFICATION_ID, Notifier.connected(this), type)
        return START_STICKY
    }

    companion object {
        private const val NOTIFICATION_ID = 7041

        fun start(context: Context) {
            val intent = Intent(context, ConnectionService::class.java)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) context.startForegroundService(intent) else context.startService(intent)
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, ConnectionService::class.java))
        }
    }
}

/**
 * UnifiedPush (https://unifiedpush.org, the Android protocol, no library):
 * the user's distributor app (ntfy, for example) hands Shodh an endpoint, which
 * the app gives to the computer over the link; the computer posts only a
 * content-free word to it. The endpoint is a secret: it waits here only until
 * the app moves it into secure storage, and is never logged.
 */
object Push {
    private const val PREFS = "shodh-push"
    private const val REGISTER = "org.unifiedpush.android.distributor.REGISTER"
    private const val UNREGISTER = "org.unifiedpush.android.distributor.UNREGISTER"

    private fun prefs(context: Context) = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    /** The distributor apps on this phone (their package names). */
    fun distributors(context: Context): List<String> =
        context.packageManager.queryBroadcastReceivers(Intent(REGISTER), 0)
            .map { it.activityInfo.packageName }
            .distinct()

    fun token(context: Context): String? = prefs(context).getString("token", null)

    fun register(context: Context, distributor: String) {
        require(distributors(context).contains(distributor)) { "That push app is not on this phone." }
        val token = token(context) ?: UUID.randomUUID().toString()
        prefs(context).edit().putString("token", token).putString("distributor", distributor).apply()
        context.sendBroadcast(
            Intent(REGISTER).setPackage(distributor)
                .putExtra("token", token)
                .putExtra("application", context.packageName),
        )
    }

    fun unregister(context: Context) {
        val prefs = prefs(context)
        val distributor = prefs.getString("distributor", null)
        val token = prefs.getString("token", null)
        if (distributor != null && token != null) {
            context.sendBroadcast(Intent(UNREGISTER).setPackage(distributor).putExtra("token", token))
        }
        prefs.edit().clear().apply()
    }

    /** A new endpoint, once: the app keeps it in secure storage. */
    fun takeEndpoint(context: Context): String? {
        val prefs = prefs(context)
        val endpoint = prefs.getString("endpoint", null)
        prefs.edit().remove("endpoint").apply()
        return endpoint
    }

    internal fun keepEndpoint(context: Context, endpoint: String) {
        prefs(context).edit().putString("endpoint", endpoint).apply()
    }
}

/** What the user's distributor app sends Shodh. Only its own token is believed. */
class PushReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val token = intent.getStringExtra("token")
        if (token == null || token != Push.token(context)) return
        when (intent.action) {
            "org.unifiedpush.android.connector.NEW_ENDPOINT" -> {
                val endpoint = intent.getStringExtra("endpoint") ?: return
                if (!endpoint.startsWith("https://")) return
                Push.keepEndpoint(context, endpoint)
                Incoming.listener?.invoke()
            }
            "org.unifiedpush.android.connector.MESSAGE" -> {
                val bytes = intent.getByteArrayExtra("bytesMessage")
                    ?: intent.getStringExtra("message")?.toByteArray()
                    ?: return
                // The body is one word; anything else is not Shodh's and is not shown.
                Notifier.post(context, String(bytes, Charsets.UTF_8).trim())
            }
            "org.unifiedpush.android.connector.UNREGISTERED" -> Push.unregister(context)
        }
    }
}

/** "A brief before your meetings": alarms a few minutes before events that have one. */
object Briefs {
    private const val PREFS = "shodh-briefs"

    private fun pending(context: Context, eventId: String): PendingIntent {
        val intent = Intent(context, BriefAlarm::class.java).setPackage(context.packageName).putExtra(Notifier.EXTRA_BRIEF, eventId)
        return PendingIntent.getBroadcast(context, eventId.hashCode(), intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    }

    /** Replace the scheduled briefs with [briefs] (event id to when, in Unix milliseconds). */
    fun schedule(context: Context, briefs: Map<String, Long>) {
        val alarms = context.getSystemService(AlarmManager::class.java) ?: return
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        for (old in prefs.getStringSet("events", emptySet()).orEmpty()) {
            if (old !in briefs) alarms.cancel(pending(context, old))
        }
        val now = System.currentTimeMillis()
        for ((eventId, at) in briefs) {
            if (at <= now) continue
            // Inexact: "about" ten minutes before, without the exact-alarm permission.
            alarms.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, at, pending(context, eventId))
        }
        prefs.edit().putStringSet("events", briefs.keys).apply()
    }
}

class BriefAlarm : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val eventId = intent.getStringExtra(Notifier.EXTRA_BRIEF) ?: return
        Notifier.post(context, "brief", eventId)
    }
}
