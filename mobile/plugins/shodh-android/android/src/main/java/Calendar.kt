package com.shodhrag.android

import android.content.ContentUris
import android.content.ContentValues
import android.content.Context
import android.database.ContentObserver
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.provider.CalendarContract
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import java.util.TimeZone

/**
 * The phone's own calendar (CalendarContract): the calendars on the phone, the
 * events of the ones the user chose, and a calendar of Shodh's own ("Shodh",
 * in the elephant's red-orange) that holds Shodh's events. Only the calendars
 * the app's Rust side names are ever read or written; each event Shodh writes
 * carries an "Open in Shodh" link (CUSTOM_APP_URI).
 */
internal object PhoneCalendar {
    const val SHODH_NAME = "Shodh"
    /** #e8482a, as Android stores a colour (ARGB). */
    const val SHODH_COLOR = 0xFFE8482A.toInt()
    private const val ACCOUNT_TYPE = CalendarContract.ACCOUNT_TYPE_LOCAL

    private fun asSyncAdapter(uri: Uri): Uri = uri.buildUpon()
        .appendQueryParameter(CalendarContract.CALLER_IS_SYNCADAPTER, "true")
        .appendQueryParameter(CalendarContract.Calendars.ACCOUNT_NAME, SHODH_NAME)
        .appendQueryParameter(CalendarContract.Calendars.ACCOUNT_TYPE, ACCOUNT_TYPE)
        .build()

    /** Every calendar on the phone, Shodh's own marked. */
    fun list(context: Context): JSArray {
        val out = JSArray()
        val columns = arrayOf(
            CalendarContract.Calendars._ID,
            CalendarContract.Calendars.CALENDAR_DISPLAY_NAME,
            CalendarContract.Calendars.ACCOUNT_NAME,
            CalendarContract.Calendars.ACCOUNT_TYPE,
            CalendarContract.Calendars.CALENDAR_COLOR,
        )
        context.contentResolver.query(CalendarContract.Calendars.CONTENT_URI, columns, null, null, null)?.use { c ->
            while (c.moveToNext()) {
                val shodh = c.getString(2) == SHODH_NAME && c.getString(3) == ACCOUNT_TYPE
                out.put(
                    JSObject()
                        .put("id", c.getLong(0))
                        .put("name", c.getString(1) ?: "Calendar")
                        .put("account", c.getString(2) ?: "")
                        .put("color", String.format("#%06x", c.getInt(4) and 0xFFFFFF))
                        .put("shodh", shodh),
                )
            }
        }
        return out
    }

    /** Shodh's own calendar, made the first time; its id. */
    fun ensureShodh(context: Context): Long {
        val selection = "${CalendarContract.Calendars.ACCOUNT_NAME} = ? AND ${CalendarContract.Calendars.ACCOUNT_TYPE} = ?"
        context.contentResolver.query(
            CalendarContract.Calendars.CONTENT_URI,
            arrayOf(CalendarContract.Calendars._ID),
            selection,
            arrayOf(SHODH_NAME, ACCOUNT_TYPE),
            null,
        )?.use { c -> if (c.moveToFirst()) return c.getLong(0) }
        val values = ContentValues().apply {
            put(CalendarContract.Calendars.ACCOUNT_NAME, SHODH_NAME)
            put(CalendarContract.Calendars.ACCOUNT_TYPE, ACCOUNT_TYPE)
            put(CalendarContract.Calendars.NAME, SHODH_NAME)
            put(CalendarContract.Calendars.CALENDAR_DISPLAY_NAME, SHODH_NAME)
            put(CalendarContract.Calendars.CALENDAR_COLOR, SHODH_COLOR)
            put(CalendarContract.Calendars.CALENDAR_ACCESS_LEVEL, CalendarContract.Calendars.CAL_ACCESS_OWNER)
            put(CalendarContract.Calendars.OWNER_ACCOUNT, SHODH_NAME)
            put(CalendarContract.Calendars.VISIBLE, 1)
            put(CalendarContract.Calendars.SYNC_EVENTS, 1)
            put(CalendarContract.Calendars.CALENDAR_TIME_ZONE, TimeZone.getDefault().id)
        }
        val uri = context.contentResolver.insert(asSyncAdapter(CalendarContract.Calendars.CONTENT_URI), values)
            ?: throw IllegalStateException("the Shodh calendar could not be made")
        return ContentUris.parseId(uri)
    }

    /** The single (not repeating) events of [calendarIds] that overlap [fromMs, toMs]. */
    fun events(context: Context, calendarIds: List<Long>, fromMs: Long, toMs: Long): JSArray {
        val out = JSArray()
        if (calendarIds.isEmpty()) return out
        val columns = arrayOf(
            CalendarContract.Events._ID,
            CalendarContract.Events.CALENDAR_ID,
            CalendarContract.Events.TITLE,
            CalendarContract.Events.DTSTART,
            CalendarContract.Events.DTEND,
            CalendarContract.Events.ALL_DAY,
            CalendarContract.Events.EVENT_LOCATION,
            CalendarContract.Events.DESCRIPTION,
            CalendarContract.Events.RRULE,
            CalendarContract.Events.CUSTOM_APP_URI,
        )
        val marks = calendarIds.joinToString(",") { "?" }
        val selection = "${CalendarContract.Events.CALENDAR_ID} IN ($marks) AND " +
            "${CalendarContract.Events.DELETED} = 0 AND " +
            "${CalendarContract.Events.DTSTART} < ? AND " +
            "(${CalendarContract.Events.DTEND} IS NULL OR ${CalendarContract.Events.DTEND} > ?)"
        val args = calendarIds.map { it.toString() } + listOf(toMs.toString(), fromMs.toString())
        context.contentResolver.query(CalendarContract.Events.CONTENT_URI, columns, selection, args.toTypedArray(), null)?.use { c ->
            while (c.moveToNext()) {
                // Repeating events are shown by the phone's calendar; Shodh keeps single events.
                if (!c.isNull(8) && c.getString(8).isNotBlank()) continue
                out.put(
                    JSObject()
                        .put("eventId", c.getLong(0))
                        .put("calendarId", c.getLong(1))
                        .put("title", c.getString(2) ?: "")
                        .put("startMs", c.getLong(3))
                        .put("endMs", if (c.isNull(4)) null else c.getLong(4))
                        .put("allDay", c.getInt(5) == 1)
                        .put("location", c.getString(6))
                        .put("description", c.getString(7) ?: "")
                        .put("shodhUri", c.getString(9)),
                )
            }
        }
        return out
    }

    private fun values(fields: JSObject, calendarId: Long?, packageName: String): ContentValues {
        val allDay = fields.getBoolean("allDay", false) ?: false
        return ContentValues().apply {
            calendarId?.let { put(CalendarContract.Events.CALENDAR_ID, it) }
            put(CalendarContract.Events.TITLE, fields.getString("title", "") ?: "")
            put(CalendarContract.Events.DESCRIPTION, fields.getString("description", "") ?: "")
            put(CalendarContract.Events.EVENT_LOCATION, fields.getString("location", null))
            put(CalendarContract.Events.DTSTART, fields.getLong("startMs"))
            if (fields.isNull("endMs")) {
                put(CalendarContract.Events.DTEND, fields.getLong("startMs") + if (allDay) 86_400_000L else 3_600_000L)
            } else {
                put(CalendarContract.Events.DTEND, fields.getLong("endMs"))
            }
            put(CalendarContract.Events.ALL_DAY, if (allDay) 1 else 0)
            put(CalendarContract.Events.EVENT_TIMEZONE, if (allDay) "UTC" else TimeZone.getDefault().id)
            fields.getString("shodhUri", null)?.let { uri ->
                put(CalendarContract.Events.CUSTOM_APP_PACKAGE, packageName)
                put(CalendarContract.Events.CUSTOM_APP_URI, uri)
            }
        }
    }

    fun insert(context: Context, calendarId: Long, fields: JSObject): Long {
        val uri = context.contentResolver.insert(
            CalendarContract.Events.CONTENT_URI,
            values(fields, calendarId, context.packageName),
        ) ?: throw IllegalStateException("the event could not be added to the phone's calendar")
        return ContentUris.parseId(uri)
    }

    /** Only an event of [calendarId] (one the user chose, or Shodh's own) is changed. */
    private val IN_CALENDAR = "${CalendarContract.Events.CALENDAR_ID} = ?"

    fun update(context: Context, calendarId: Long, eventId: Long, fields: JSObject) {
        val uri = ContentUris.withAppendedId(CalendarContract.Events.CONTENT_URI, eventId)
        context.contentResolver.update(uri, values(fields, null, context.packageName), IN_CALENDAR, arrayOf(calendarId.toString()))
    }

    fun delete(context: Context, calendarId: Long, eventId: Long) {
        val uri = ContentUris.withAppendedId(CalendarContract.Events.CONTENT_URI, eventId)
        context.contentResolver.delete(uri, IN_CALENDAR, arrayOf(calendarId.toString()))
    }

    /** Tell [changed] when the phone's calendars change, while the app runs. */
    fun watch(context: Context, changed: () -> Unit): ContentObserver {
        val observer = object : ContentObserver(Handler(Looper.getMainLooper())) {
            override fun onChange(selfChange: Boolean) {
                changed()
            }
        }
        context.contentResolver.registerContentObserver(CalendarContract.Events.CONTENT_URI, true, observer)
        return observer
    }
}
