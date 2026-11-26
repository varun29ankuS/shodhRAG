import Foundation
import UIKit
import UserNotifications

/// Shodh's notifications. Each says only what kind of thing waits ("Shodh
/// needs your approval", "Your answer is ready"), never a question, an answer,
/// a title or a name: the details come over the encrypted link when the app
/// opens. Nothing is shown while the app is on screen (iOS shows an app's
/// notification over the app only when its delegate asks, and this one never
/// does).
final class Notifier: NSObject, UNUserNotificationCenterDelegate {
  /// The key of a brief's event in its notification.
  private static let briefKey = "shodhBrief"

  /// The start of every brief's request identifier.
  private static let briefPrefix = "shodh-brief-"

  /// A brief's notification was tapped: its event.
  var onBrief: ((String) -> Void)?

  private let center = UNUserNotificationCenter.current()

  /// "Stay connected" is chosen (main thread only).
  private var stays = false

  /// iOS's permission to keep running after the app left the screen (main thread only).
  private var background = UIBackgroundTaskIdentifier.invalid

  override init() {
    super.init()
    center.delegate = self
    let notifications = NotificationCenter.default
    notifications.addObserver(
      self, selector: #selector(leftScreen), name: UIApplication.didEnterBackgroundNotification, object: nil)
    notifications.addObserver(
      self, selector: #selector(backOnScreen), name: UIApplication.willEnterForegroundNotification, object: nil)
  }

  /// The words of each kind; anything else is not posted.
  private static func title(_ kind: String) -> String? {
    switch kind {
    case "approval": return "Shodh needs your approval"
    case "answer": return "Your answer is ready"
    case "reminder": return "You have a reminder in Shodh"
    case "brief": return "Your meeting brief is ready"
    default: return nil
    }
  }

  /// Whether iOS lets Shodh notify, asking the user the first time.
  func access(_ done: @escaping (Bool) -> Void) {
    center.getNotificationSettings { settings in
      switch settings.authorizationStatus {
      case .notDetermined:
        self.center.requestAuthorization(options: [.alert, .sound]) { granted, _ in
          done(granted)
        }
      case .denied:
        done(false)
      default:
        done(true)
      }
    }
  }

  /// Post a notification of `kind` now (the app's Rust side, from the link).
  func post(_ kind: String, done: @escaping (String?) -> Void) {
    guard let title = Notifier.title(kind) else {
      done(nil)
      return
    }
    DispatchQueue.main.async {
      if UIApplication.shared.applicationState == .active {
        done(nil)
        return
      }
      let content = UNMutableNotificationContent()
      content.title = title
      content.sound = .default
      // One of each kind at a time: a second approval replaces the first.
      let request = UNNotificationRequest(identifier: "shodh-" + kind, content: content, trigger: nil)
      self.center.add(request) { error in
        done(error?.localizedDescription)
      }
    }
  }

  /// Replace the scheduled briefs with `briefs` (event id to when, in Unix milliseconds).
  func schedule(_ briefs: [(String, Int64)], done: @escaping (String?) -> Void) {
    center.getPendingNotificationRequests { pending in
      let old = pending.map { $0.identifier }.filter { $0.hasPrefix(Notifier.briefPrefix) }
      self.center.removePendingNotificationRequests(withIdentifiers: old)
      let now = Date().timeIntervalSince1970
      var problem: String?
      let group = DispatchGroup()
      for (eventId, atMs) in briefs {
        let after = Double(atMs) / 1000 - now
        if after <= 0 { continue }
        let content = UNMutableNotificationContent()
        content.title = Notifier.title("brief") ?? ""
        content.sound = .default
        content.userInfo = [Notifier.briefKey: eventId]
        let trigger = UNTimeIntervalNotificationTrigger(timeInterval: max(1, after), repeats: false)
        let request = UNNotificationRequest(
          identifier: Notifier.briefPrefix + eventId, content: content, trigger: trigger)
        group.enter()
        self.center.add(request) { error in
          if let error = error { problem = error.localizedDescription }
          group.leave()
        }
      }
      group.notify(queue: .main) {
        done(problem)
      }
    }
  }

  // ── Staying connected ───────────────────────────────────────────

  /// "Stay connected" on iPhone: when the app leaves the screen, Shodh asks
  /// iOS for the time it gives an app to finish (about half a minute), so the
  /// link stays open that long and an approval or an answer that arrives in it
  /// is announced. iOS then suspends the app: there is no lasting connection
  /// on iPhone, and the window says so.
  func stay(_ on: Bool) {
    DispatchQueue.main.async {
      self.stays = on
      if !on { self.endBackground() }
    }
  }

  @objc private func leftScreen() {
    guard stays, background == .invalid else { return }
    background = UIApplication.shared.beginBackgroundTask(withName: "shodh-link") { [weak self] in
      self?.endBackground()
    }
  }

  @objc private func backOnScreen() {
    endBackground()
  }

  private func endBackground() {
    guard background != .invalid else { return }
    UIApplication.shared.endBackgroundTask(background)
    background = .invalid
  }

  // ── UNUserNotificationCenterDelegate ────────────────────────────

  func userNotificationCenter(
    _ center: UNUserNotificationCenter,
    didReceive response: UNNotificationResponse,
    withCompletionHandler completionHandler: @escaping () -> Void
  ) {
    if let eventId = response.notification.request.content.userInfo[Notifier.briefKey] as? String {
      onBrief?(eventId)
    }
    completionHandler()
  }
}
