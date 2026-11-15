package com.shodhrag.android

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import java.io.File
import java.util.UUID

/**
 * Files going to the computer: what the user picked, photographed or shared is
 * copied at once into the app's own cache (`outgoing/`), because the
 * permission to read a shared or picked file ends with the app. The copy is
 * what the outbox keeps and sends; it is removed once the computer has it.
 */
internal object Files {
    /** The largest file copied (the computer's largest limit, for PDFs). */
    const val MAX_BYTES = 512L * 1024 * 1024

    fun outgoing(context: Context): File = File(context.cacheDir, "outgoing")

    /** A new folder for one file, so two files of one name stay apart. */
    fun newFolder(context: Context): File {
        val folder = File(outgoing(context), UUID.randomUUID().toString())
        folder.mkdirs()
        return folder
    }

    /** A plain file name: no folders, no control characters, never empty. */
    fun plainName(raw: String?, fallback: String): String {
        val name = (raw ?: "").substringAfterLast('/').substringAfterLast('\\')
            .filter { it >= ' ' && it != ':' }
            .trim()
            .trim('.')
        return if (name.isEmpty()) fallback else name.take(200)
    }

    fun displayName(context: Context, uri: Uri): String? {
        if (uri.scheme == "file") return uri.lastPathSegment
        context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
            if (cursor.moveToFirst()) {
                val index = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                if (index >= 0) return cursor.getString(index)
            }
        }
        return null
    }

    /** Copy [uri] into a new folder of `outgoing/`; its path and its name. */
    fun copy(context: Context, uri: Uri): Pair<File, String> {
        val name = plainName(displayName(context, uri), "file")
        val target = File(newFolder(context), name)
        val input = context.contentResolver.openInputStream(uri)
            ?: throw IllegalStateException("$name could not be read")
        input.use { source ->
            target.outputStream().use { out ->
                val buffer = ByteArray(64 * 1024)
                var total = 0L
                while (true) {
                    val read = source.read(buffer)
                    if (read < 0) break
                    total += read
                    if (total > MAX_BYTES) {
                        target.delete()
                        throw IllegalStateException("$name is larger than Shodh can read")
                    }
                    out.write(buffer, 0, read)
                }
                out.fd.sync()
            }
        }
        return Pair(target, name)
    }
}
