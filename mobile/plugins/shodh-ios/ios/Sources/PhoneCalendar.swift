import CryptoKit
import EventKit
import Foundation
import Tauri
import UIKit

/// The fields of an event as the window sends them (mobile/src/lib/calsync.ts,
/// `toPhone`): all-day events are UTC midnights, the end the day after the
/// last day, as Android's calendar keeps them.
struct EventFields: Decodable {
  let title: String?
  let description: String?
  let location: String?
  let startMs: Int64
  let endMs: Int64?
  let allDay: Bool?
  let shodhUri: String?
}

/// The phone's own calendar (EventKit): the calendars on the phone, the
/// events of the ones the user chose, and a calendar of Shodh's own ("Shodh",
/// in the elephant's red-orange) that holds Shodh's events. Only the calendars
/// the app's Rust side names are ever read or written; each event Shodh
/// writes carries an "Open in Shodh" link (its URL).
///
/// The window and the app's Rust side name calendars and events by number,
/// as Android does. EventKit names them by text, so each number here is the
/// first 53 bits of the SHA-256 of that text: the same number for the same
/// calendar or event every time, and one JavaScript holds exactly.
final class PhoneCalendar {
  static let shodhName = "Shodh"

  /// Where the identifier of Shodh's own calendar is kept.
  private static let shodhKey = "shodh.calendar"

  private let store = EKEventStore()
  private let lock = NSLock()

  /// The identifier of each event seen or written, by number (guarded by `lock`).
  private var seen: [Int64: String] = [:]

  private var watching = false

  // ── Permission ──────────────────────────────────────────────────

  /// Whether Shodh may read and write the phone's calendars.
  static func granted() -> Bool {
    let status = EKEventStore.authorizationStatus(for: .event)
    if #available(iOS 17.0, *) {
      return status == .fullAccess
    }
    return status == .authorized
  }

  /// Ask for the calendars (iOS asks the user once; after a no, only Settings changes it).
  func access(_ done: @escaping (Bool) -> Void) {
    if PhoneCalendar.granted() {
      done(true)
      return
    }
    if #available(iOS 17.0, *) {
      store.requestFullAccessToEvents { granted, _ in done(granted) }
    } else {
      store.requestAccess(to: .event) { granted, _ in done(granted) }
    }
  }

  /// Tell `changed` when the phone's calendars change, while the app runs.
  func watch(_ changed: @escaping () -> Void) {
    lock.lock()
    let already = watching
    watching = true
    lock.unlock()
    if already { return }
    NotificationCenter.default.addObserver(forName: .EKEventStoreChanged, object: store, queue: .main) { _ in
      changed()
    }
  }

  // ── Numbers ─────────────────────────────────────────────────────

  static func number(_ identifier: String) -> Int64 {
    let digest = SHA256.hash(data: Data(identifier.utf8))
    var value: UInt64 = 0
    for byte in digest.prefix(8) {
      value = (value << 8) | UInt64(byte)
    }
    return Int64(value >> 11)
  }

  private func findCalendar(_ number: Int64) -> EKCalendar? {
    store.calendars(for: .event).first { PhoneCalendar.number($0.calendarIdentifier) == number }
  }

  private func remember(_ event: EKEvent) -> Int64? {
    guard let identifier = event.eventIdentifier else { return nil }
    let number = PhoneCalendar.number(identifier)
    lock.lock()
    seen[number] = identifier
    lock.unlock()
    return number
  }

  /// The event numbered `number` in `calendar`: one seen or written since the
  /// app started, or else looked for among the calendar's events of the last
  /// year and the next three (the widest span EventKit searches at once).
  private func findEvent(_ number: Int64, in calendar: EKCalendar) -> EKEvent? {
    lock.lock()
    let known = seen[number]
    lock.unlock()
    if let known = known, let kept = store.event(withIdentifier: known) {
      if kept.calendar?.calendarIdentifier == calendar.calendarIdentifier {
        return kept
      }
    }
    let now = Date()
    let predicate = store.predicateForEvents(
      withStart: now.addingTimeInterval(-365 * 86_400), end: now.addingTimeInterval(3 * 365 * 86_400),
      calendars: [calendar])
    return store.events(matching: predicate).first { found in
      guard let identifier = found.eventIdentifier else { return false }
      return PhoneCalendar.number(identifier) == number
    }
  }

  // ── Days ────────────────────────────────────────────────────────

  private static var utc: Calendar {
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = TimeZone(identifier: "UTC") ?? TimeZone(secondsFromGMT: 0)!
    return calendar
  }

  private static var local: Calendar {
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = TimeZone.current
    return calendar
  }

  private static func date(_ ms: Int64) -> Date {
    Date(timeIntervalSince1970: Double(ms) / 1000)
  }

  private static func ms(_ date: Date) -> Int64 {
    Int64((date.timeIntervalSince1970 * 1000).rounded())
  }

  /// The start of the local day that the UTC midnight `ms` names.
  private static func localDay(ofUTC ms: Int64) -> Date {
    let day = utc.dateComponents([.year, .month, .day], from: date(ms))
    return local.date(from: day) ?? date(ms)
  }

  /// The UTC midnight of the local day `date` is in.
  private static func utcDay(ofLocal date: Date) -> Int64 {
    let day = local.dateComponents([.year, .month, .day], from: date)
    return ms(utc.date(from: day) ?? date)
  }

  // ── Calendars ───────────────────────────────────────────────────

  private static func hex(_ color: CGColor?) -> String {
    var red: CGFloat = 0
    var green: CGFloat = 0
    var blue: CGFloat = 0
    var alpha: CGFloat = 0
    guard let color = color, UIColor(cgColor: color).getRed(&red, green: &green, blue: &blue, alpha: &alpha) else {
      return "#888888"
    }
    let part = { (value: CGFloat) -> Int in Int((min(max(value, 0), 1) * 255).rounded()) }
    return String(format: "#%02lx%02lx%02lx", part(red), part(green), part(blue))
  }

  private func shodhIdentifier() -> String? {
    guard let kept = UserDefaults.standard.string(forKey: PhoneCalendar.shodhKey),
      store.calendar(withIdentifier: kept) != nil
    else {
      return nil
    }
    return kept
  }

  /// Every calendar on the phone, Shodh's own marked.
  func list() -> [JsonObject] {
    let own = shodhIdentifier()
    return store.calendars(for: .event).map { calendar -> JsonObject in
      return [
        "id": PhoneCalendar.number(calendar.calendarIdentifier),
        "name": calendar.title,
        "account": calendar.source?.title ?? "",
        "color": PhoneCalendar.hex(calendar.cgColor),
        "shodh": calendar.calendarIdentifier == own,
      ]
    }
  }

  /// Shodh's own calendar, made the first time; its number. It is made where
  /// the phone keeps new events (the account of its default calendar), else
  /// on the phone itself, else in any account that takes a calendar.
  func ensureShodh() throws -> Int64 {
    if let kept = shodhIdentifier() {
      return PhoneCalendar.number(kept)
    }
    var sources: [EKSource] = []
    if let preferred = store.defaultCalendarForNewEvents?.source {
      sources.append(preferred)
    }
    sources.append(contentsOf: store.sources.filter { $0.sourceType == .local })
    sources.append(contentsOf: store.sources.filter { $0.sourceType == .calDAV || $0.sourceType == .exchange })
    var failure: Error?
    for source in sources {
      let calendar = EKCalendar(for: .event, eventStore: store)
      calendar.title = PhoneCalendar.shodhName
      calendar.cgColor = UIColor(red: 0xE8 / 255.0, green: 0x48 / 255.0, blue: 0x2A / 255.0, alpha: 1).cgColor
      calendar.source = source
      do {
        try store.saveCalendar(calendar, commit: true)
        UserDefaults.standard.set(calendar.calendarIdentifier, forKey: PhoneCalendar.shodhKey)
        return PhoneCalendar.number(calendar.calendarIdentifier)
      } catch {
        failure = error
      }
    }
    throw Problem("The Shodh calendar could not be made: \(failure?.localizedDescription ?? "this iPhone has no account that keeps calendars")")
  }

  // ── Events ──────────────────────────────────────────────────────

  /// The single (not repeating) events of `calendars` that overlap [fromMs, toMs].
  func events(calendars numbers: [Int64], fromMs: Int64, toMs: Int64) -> [JsonObject] {
    let wanted = Set(numbers)
    let calendars = store.calendars(for: .event).filter { wanted.contains(PhoneCalendar.number($0.calendarIdentifier)) }
    if calendars.isEmpty || toMs <= fromMs { return [] }
    let predicate = store.predicateForEvents(
      withStart: PhoneCalendar.date(fromMs), end: PhoneCalendar.date(toMs), calendars: calendars)
    var out: [JsonObject] = []
    for event in store.events(matching: predicate) {
      // Repeating events are shown by the phone's calendar; Shodh keeps single events.
      if event.hasRecurrenceRules || event.isDetached { continue }
      guard let number = remember(event), let calendar = event.calendar else { continue }
      let startMs: Int64
      let endMs: Int64
      if event.isAllDay {
        startMs = PhoneCalendar.utcDay(ofLocal: event.startDate)
        endMs = PhoneCalendar.utcDay(ofLocal: event.endDate) + 86_400_000
      } else {
        startMs = PhoneCalendar.ms(event.startDate)
        endMs = PhoneCalendar.ms(event.endDate)
      }
      let link = event.url?.absoluteString
      out.append([
        "eventId": number,
        "calendarId": PhoneCalendar.number(calendar.calendarIdentifier),
        "title": event.title ?? "",
        "startMs": startMs,
        "endMs": endMs,
        "allDay": event.isAllDay,
        "location": PhoneCalendar.orNull(event.location),
        "description": event.notes ?? "",
        "shodhUri": PhoneCalendar.orNull(link?.hasPrefix(Outgoing.eventLink) == true ? link : nil),
      ])
    }
    return out
  }

  /// JSON's null for a text that is not there.
  private static func orNull(_ value: String?) -> Any {
    if let value = value { return value }
    return NSNull()
  }

  private func apply(_ fields: EventFields, to event: EKEvent) {
    let allDay = fields.allDay ?? false
    event.title = fields.title ?? ""
    event.notes = fields.description ?? ""
    event.location = fields.location
    event.isAllDay = allDay
    if allDay {
      let start = PhoneCalendar.localDay(ofUTC: fields.startMs)
      // The window's end is the UTC midnight after the last day; EventKit's
      // is a moment of the last day itself.
      let last = fields.endMs.map { PhoneCalendar.localDay(ofUTC: $0 - 86_400_000) } ?? start
      event.startDate = start
      event.endDate = max(last, start)
    } else {
      let start = PhoneCalendar.date(fields.startMs)
      event.startDate = start
      event.endDate = fields.endMs.map(PhoneCalendar.date) ?? start.addingTimeInterval(3600)
      event.timeZone = TimeZone.current
    }
    if let link = fields.shodhUri, let url = URL(string: link) {
      event.url = url
    }
  }

  func insert(calendar number: Int64, fields: EventFields) throws -> Int64 {
    guard let target = findCalendar(number) else {
      throw Problem("That calendar is not on this iPhone any more.")
    }
    let event = EKEvent(eventStore: store)
    event.calendar = target
    apply(fields, to: event)
    do {
      try store.save(event, span: .thisEvent, commit: true)
    } catch {
      throw Problem("The event could not be added to the phone's calendar: \(error.localizedDescription)")
    }
    guard let made = remember(event) else {
      throw Problem("The event could not be added to the phone's calendar.")
    }
    return made
  }

  /// Only an event of calendar `number` (one the user chose, or Shodh's own) is changed.
  func update(calendar number: Int64, event eventNumber: Int64, fields: EventFields) throws {
    guard let target = findCalendar(number), let event = findEvent(eventNumber, in: target) else { return }
    apply(fields, to: event)
    do {
      try store.save(event, span: .thisEvent, commit: true)
    } catch {
      throw Problem("The event could not be changed in the phone's calendar: \(error.localizedDescription)")
    }
  }

  func delete(calendar number: Int64, event eventNumber: Int64) throws {
    guard let target = findCalendar(number), let event = findEvent(eventNumber, in: target) else { return }
    do {
      try store.remove(event, span: .thisEvent, commit: true)
    } catch {
      throw Problem("The event could not be removed from the phone's calendar: \(error.localizedDescription)")
    }
    lock.lock()
    seen[eventNumber] = nil
    lock.unlock()
  }
}
