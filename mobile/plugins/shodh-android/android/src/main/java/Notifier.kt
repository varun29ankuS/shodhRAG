package com.shodhrag.android

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat

/**
 * Shodh's notifications. Each says only what kind of thing waits ("Shodh
 * needs your approval", "Your answer is ready"), never a question, an answer,
 * a title or a name: the details come over the encrypted link when the app
 * opens. Nothing is shown while the app is on screen.
 */
object Notifier {
    private const val CHANNEL = "shodh"
    private const val SERVICE_CHANNEL = "shodh-connected"
    const val EXTRA_BRIEF = "shodh.brief"

    /** The app is on screen (set by the plugin as the app resumes and pauses). */
    @Volatile
    @JvmStatic
    var foreground = false

    /** The words of each kind; anything else is not posted. */
    private fun title(kind: String): String? = when (kind) {
        "approval" -> "Shodh needs your approval"
        "answer" -> "Your answer is ready"
        "reminder" -> "You have a reminder in Shodh"
        "brief" -> "Your meeting brief is ready"
        else -> null
    }

    private fun channels(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        manager.createNotificationChannel(NotificationChannel(CHANNEL, "Approvals and answers", NotificationManager.IMPORTANCE_DEFAULT))
        manager.createNotificationChannel(NotificationChannel(SERVICE_CHANNEL, "Connected to your computer", NotificationManager.IMPORTANCE_MIN))
    }

    private fun open(context: Context, briefEvent: String?): PendingIntent? {
        val launch = context.packageManager.getLaunchIntentForPackage(context.packageName) ?: return null
        launch.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        briefEvent?.let { launch.putExtra(EXTRA_BRIEF, it) }
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        return PendingIntent.getActivity(context, briefEvent?.hashCode() ?: 0, launch, flags)
    }

    /** Post a notification of [kind] (the app's Rust side, a push, or a brief's alarm). */
    @JvmStatic
    fun post(context: Context, kind: String) = post(context, kind, null)

    @JvmStatic
    fun post(context: Context, kind: String, briefEvent: String?) {
        val text = title(kind) ?: return
        if (foreground) return
        channels(context)
        val notification = NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(context.applicationInfo.icon)
            .setContentTitle(text)
            .setAutoCancel(true)
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .setContentIntent(open(context, briefEvent))
            .build()
        val manager = NotificationManagerCompat.from(context)
        if (!manager.areNotificationsEnabled()) return
        try {
            manager.notify(kind.hashCode() + (briefEvent?.hashCode() ?: 0), notification)
        } catch (_: SecurityException) {
            // Notifications are not allowed: nothing is shown.
        }
    }

    /** The lasting notification of "Stay connected". */
    fun connected(context: Context) = run {
        channels(context)
        NotificationCompat.Builder(context, SERVICE_CHANNEL)
            .setSmallIcon(context.applicationInfo.icon)
            .setContentTitle("Shodh is connected")
            .setContentText("Shodh tells you when your computer needs you.")
            .setOngoing(true)
            .setContentIntent(open(context, null))
            .build()
    }
}
