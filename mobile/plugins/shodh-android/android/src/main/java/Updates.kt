package com.shodhrag.android

import android.app.Activity
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInfo
import android.content.pm.PackageInstaller
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.Settings
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.FileOutputStream
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest

/**
 * The one-tap update's Android side: downloads (HTTPS only, into the app's own
 * storage) and the hand-over to Android's package installer. The app's Rust
 * side decides what is downloaded and verifies the file (Shodh's release key
 * and the release's checksum) before it asks for [install]; this side checks
 * again that the file is this app, signed by the same key as the installed one.
 */
internal object Updates {
    private const val TIMEOUT_MS = 30_000
    private const val MAX_TEXT_BYTES = 1L shl 20

    /** [path], refused unless it is inside this app's own storage. */
    fun ownFile(context: Context, path: String): File {
        val file = File(path).canonicalFile
        // The app's private folder (its files and its cache are inside it).
        val root = context.dataDir.canonicalFile
        require(file.path.startsWith(root.path + File.separator)) {
            "Shodh only writes inside its own storage."
        }
        return file
    }

    private fun open(url: String): HttpURLConnection {
        require(url.startsWith("https://")) { "Shodh downloads over HTTPS only." }
        val connection = URL(url).openConnection() as HttpURLConnection
        connection.connectTimeout = TIMEOUT_MS
        connection.readTimeout = TIMEOUT_MS
        connection.instanceFollowRedirects = true
        connection.setRequestProperty("User-Agent", "Shodh")
        connection.setRequestProperty("Accept", "application/vnd.github+json, application/octet-stream")
        val status = connection.responseCode
        if (status == 404) throw IllegalStateException("not found")
        if (status !in 200..299) throw IllegalStateException("the server answered $status")
        // HttpURLConnection never follows a redirect to plain HTTP; the end is checked anyway.
        require(connection.url.protocol == "https") { "Shodh downloads over HTTPS only." }
        return connection
    }

    /** A small text document (a release's description). */
    fun text(url: String): String {
        val connection = open(url)
        try {
            val out = ByteArrayOutputStream()
            connection.inputStream.use { input ->
                val buffer = ByteArray(16 * 1024)
                var total = 0L
                while (true) {
                    val read = input.read(buffer)
                    if (read < 0) break
                    total += read
                    if (total > MAX_TEXT_BYTES) throw IllegalStateException("the answer is too long")
                    out.write(buffer, 0, read)
                }
            }
            return String(out.toByteArray(), Charsets.UTF_8)
        } finally {
            connection.disconnect()
        }
    }

    /** Download [url] to [path] (in the app's storage), at most [maxBytes]; the bytes written. */
    fun download(context: Context, url: String, path: String, maxBytes: Long): Long {
        val target = ownFile(context, path)
        target.parentFile?.mkdirs()
        val partial = File(target.path + ".partial")
        val connection = open(url)
        try {
            var total = 0L
            connection.inputStream.use { input ->
                FileOutputStream(partial).use { out ->
                    val buffer = ByteArray(64 * 1024)
                    while (true) {
                        val read = input.read(buffer)
                        if (read < 0) break
                        total += read
                        if (total > maxBytes) throw IllegalStateException("the file is larger than expected")
                        out.write(buffer, 0, read)
                    }
                    out.fd.sync()
                }
            }
            if (!partial.renameTo(target)) throw IllegalStateException("the download could not be kept")
            return total
        } finally {
            partial.delete()
            connection.disconnect()
        }
    }

    private fun sha256(bytes: ByteArray): String =
        MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }

    @Suppress("DEPRECATION")
    private fun signers(info: PackageInfo?): Set<String> {
        if (info == null) return emptySet()
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            val signing = info.signingInfo ?: return emptySet()
            val certificates =
                if (signing.hasMultipleSigners()) signing.apkContentsSigners else signing.signingCertificateHistory
            certificates.map { sha256(it.toByteArray()) }.toSet()
        } else {
            info.signatures?.map { sha256(it.toByteArray()) }?.toSet() ?: emptySet()
        }
    }

    @Suppress("DEPRECATION")
    private fun signatureFlags(): Int =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) PackageManager.GET_SIGNING_CERTIFICATES
        else PackageManager.GET_SIGNATURES

    /**
     * Hand the verified app at [path] to Android's installer. Refused unless it
     * is this app (the same package) signed by the same key as the installed one.
     * False when Android first needs the user to allow Shodh to install updates
     * (that settings page is opened).
     */
    fun install(activity: Activity, path: String): Boolean {
        val file = ownFile(activity, path)
        val manager = activity.packageManager
        val archive = manager.getPackageArchiveInfo(file.path, signatureFlags())
            ?: throw IllegalStateException("the file is not an Android app")
        if (archive.packageName != activity.packageName) {
            throw IllegalStateException("the file is another app")
        }
        val installed = signers(manager.getPackageInfo(activity.packageName, signatureFlags()))
        val offered = signers(archive)
        if (offered.isEmpty() || installed.isEmpty() || offered.intersect(installed).isEmpty()) {
            throw IllegalStateException("it is not signed by the key this Shodh was installed with")
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && !manager.canRequestPackageInstalls()) {
            val settings = Intent(
                Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                Uri.parse("package:${activity.packageName}"),
            )
            activity.startActivity(settings)
            return false
        }
        val installer = manager.packageInstaller
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL)
        params.setAppPackageName(activity.packageName)
        val id = installer.createSession(params)
        installer.openSession(id).use { session ->
            session.openWrite("shodh.apk", 0, file.length()).use { out ->
                file.inputStream().use { it.copyTo(out) }
                session.fsync(out)
            }
            val intent = Intent(activity, InstallResult::class.java).setPackage(activity.packageName)
            val mutable = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) PendingIntent.FLAG_MUTABLE else 0
            val pending = PendingIntent.getBroadcast(activity, id, intent, PendingIntent.FLAG_UPDATE_CURRENT or mutable)
            session.commit(pending.intentSender)
        }
        return true
    }
}

/** Android's installer reports here; it first asks the user to confirm. */
class InstallResult : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)
        if (status != PackageInstaller.STATUS_PENDING_USER_ACTION) return
        val confirm: Intent? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java)
        } else {
            @Suppress("DEPRECATION")
            intent.getParcelableExtra(Intent.EXTRA_INTENT)
        }
        confirm?.let {
            it.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            context.startActivity(it)
        }
    }
}
