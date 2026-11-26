import AVFoundation
import Foundation
import Tauri
import UIKit
import UniformTypeIdentifiers
import WebKit

struct BarsArgs: Decodable {
  let dark: Bool
  /// The app follows the phone's own light or dark (its default), rather than a theme the user pinned.
  let followsPhone: Bool?
}

struct FolderArgs: Decodable {
  let folder: String
}

struct OpenedArgs: Decodable {
  let folder: String
  let urls: [String]
}

struct KindArgs: Decodable {
  let kind: String
}

struct TextArgs: Decodable {
  let text: String
}

struct Brief: Decodable {
  let eventId: String
  let atMs: Int64
}

struct BriefsArgs: Decodable {
  let briefs: [Brief]
}

struct EventsArgs: Decodable {
  let calendarIds: [Int64]
  let fromMs: Int64
  let toMs: Int64
}

struct InsertArgs: Decodable {
  let calendarId: Int64
  let fields: EventFields
}

struct UpdateArgs: Decodable {
  let calendarId: Int64
  let eventId: Int64
  let fields: EventFields
}

struct DeleteArgs: Decodable {
  let calendarId: Int64
  let eventId: Int64
}

/// Lies over the web view, sees nothing and takes no touch: UIKit tells it
/// when the safe area changes (a rotation, a call's taller status bar).
final class SafeAreaWatch: UIView {
  var changed: (() -> Void)?

  override func safeAreaInsetsDidChange() {
    super.safeAreaInsetsDidChange()
    changed?()
  }
}

/// Shodh's iOS glue. Commands are called by the app's Rust side (never
/// granted to the window), which checks what it asks for; the window only
/// listens to this plugin's events. The commands and their answers are the
/// Android plugin's (plugins/shodh-android, ShodhPlugin.kt), so the app's
/// Rust side is one for both phones.
class ShodhPlugin: Plugin {
  /// The app's web view, whose safe area the page is told.
  private weak var webView: WKWebView?

  /// The safe area as the page was last told (main thread only).
  private var insets = UIEdgeInsets.zero

  private let incoming = Incoming()
  private let notifier = Notifier()
  /// Made when first used: neither is needed to start the app.
  private lazy var calendar = PhoneCalendar()
  private lazy var voice = Voice()

  /// The picker or the camera that is open, kept until it answers (main thread only).
  private var picker: PickerAnswer?
  private var camera: CameraAnswer?

  public override func load(webview: WKWebView) {
    webView = webview
    // The page draws edge to edge (viewport-fit=cover) and keeps clear of the
    // bars itself (theme.css --inset-*): the web view adds no inset of its own.
    webview.scrollView.contentInsetAdjustmentBehavior = .never
    let watch = SafeAreaWatch(frame: webview.bounds)
    watch.autoresizingMask = [.flexibleWidth, .flexibleHeight]
    watch.isUserInteractionEnabled = false
    watch.backgroundColor = .clear
    watch.changed = { [weak self] in self?.tellInsets() }
    webview.addSubview(watch)
    notifier.onBrief = { [weak self] eventId in
      // A brief's notification was tapped: the app opens at that brief.
      self?.arrived(["kind": "brief", "eventId": eventId])
    }
    watchCalendar()
  }

  /// Keep what arrived for the window, and tell it.
  private func arrived(_ payload: JsonObject) {
    incoming.deliver(payload)
    let nothing: JSObject = [:]
    trigger("incoming", data: nothing)
  }

  private func watchCalendar() {
    guard PhoneCalendar.granted() else { return }
    calendar.watch { [weak self] in
      let nothing: JSObject = [:]
      self?.trigger("calendar", data: nothing)
    }
  }

  /// Run `work` and answer `invoke` with its result or its error's words.
  private func answer(_ invoke: Invoke, _ work: () throws -> JsonObject) {
    do {
      invoke.resolve(try work())
    } catch {
      invoke.reject(error.localizedDescription)
    }
  }

  private func present(_ controller: UIViewController, for invoke: Invoke) -> Bool {
    guard let root = manager.viewController else {
      invoke.reject("Shodh's window is not on screen.")
      return false
    }
    var top = root
    while let above = top.presentedViewController { top = above }
    top.present(controller, animated: true)
    return true
  }

  // ── The status bar and the safe area ─────────────────────────────

  /// Light status bar text on the dark theme, dark on the light one. iOS
  /// draws the status bar for the window's own light or dark: the window
  /// follows the phone while the app does, and is pinned when the user
  /// pinned a theme. (A pinned window also tells the page that theme as the
  /// phone's, which the page does not read while a theme is pinned.)
  @objc public func setSystemBars(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(BarsArgs.self)
    DispatchQueue.main.async {
      let style: UIUserInterfaceStyle = (args.followsPhone ?? false) ? .unspecified : (args.dark ? .dark : .light)
      self.webView?.window?.overrideUserInterfaceStyle = style
      invoke.resolve()
    }
  }

  private static func json(_ insets: UIEdgeInsets) -> JsonObject {
    [
      "top": Double(insets.top), "right": Double(insets.right),
      "bottom": Double(insets.bottom), "left": Double(insets.left),
    ]
  }

  /// The status bar or Dynamic Island, the home indicator and, on its side,
  /// the sensor housing: where they cover the web view, in points, which
  /// are the page's CSS pixels.
  private func tellInsets() {
    guard let now = webView?.safeAreaInsets, now != insets else { return }
    insets = now
    NSLog(
      "Shodh system bars: top %.1f pt, right %.1f pt, bottom %.1f pt, left %.1f pt",
      Double(now.top), Double(now.right), Double(now.bottom), Double(now.left))
    let data: JSObject = [
      "top": Double(now.top), "right": Double(now.right),
      "bottom": Double(now.bottom), "left": Double(now.left),
    ]
    trigger("insets", data: data)
  }

  @objc public func systemInsets(_ invoke: Invoke) {
    DispatchQueue.main.async {
      invoke.resolve(ShodhPlugin.json(self.webView?.safeAreaInsets ?? self.insets))
    }
  }

  // ── Files: Add, the camera, what is opened with Shodh ────────────

  @objc public func pickFiles(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(FolderArgs.self)
    DispatchQueue.main.async {
      let chooser: UIDocumentPickerViewController
      if #available(iOS 14.0, *) {
        // asCopy: iOS hands over its own copy of each file, readable after the picker has gone.
        chooser = UIDocumentPickerViewController(forOpeningContentTypes: [UTType.item], asCopy: true)
      } else {
        chooser = UIDocumentPickerViewController(documentTypes: ["public.item"], in: .import)
      }
      chooser.allowsMultipleSelection = true
      let answer = PickerAnswer { [weak self] urls in
        self?.picker = nil
        // Copying runs off the main thread: a large file takes a moment.
        DispatchQueue.global(qos: .userInitiated).async {
          self?.answer(invoke) {
            // iOS's own copies are not needed once Shodh has made its own.
            defer { urls.forEach(Outgoing.removeSystemCopy) }
            return ["files": try Outgoing.copies(urls, into: args.folder)]
          }
        }
      }
      chooser.delegate = answer
      if self.present(chooser, for: invoke) {
        chooser.presentationController?.delegate = answer
        self.picker = answer
      }
    }
  }

  @objc public func takePhoto(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(FolderArgs.self)
    DispatchQueue.main.async {
      guard UIImagePickerController.isSourceTypeAvailable(.camera) else {
        invoke.reject("This iPhone has no camera Shodh can use.")
        return
      }
      switch AVCaptureDevice.authorizationStatus(for: .video) {
      case .authorized:
        self.openCamera(invoke, folder: args.folder)
      case .notDetermined:
        AVCaptureDevice.requestAccess(for: .video) { granted in
          if granted {
            self.openCamera(invoke, folder: args.folder)
          } else {
            invoke.reject("Shodh needs the camera to take a photo. Allow it in Settings, Shodh.")
          }
        }
      default:
        invoke.reject("Shodh needs the camera to take a photo. Allow it in Settings, Shodh.")
      }
    }
  }

  private func openCamera(_ invoke: Invoke, folder: String) {
    DispatchQueue.main.async {
      let camera = UIImagePickerController()
      camera.sourceType = .camera
      let answer = CameraAnswer { [weak self] image in
        self?.camera = nil
        guard let image = image else {
          // The user went back: no photo.
          let none: JsonObject = [:]
          invoke.resolve(none)
          return
        }
        DispatchQueue.global(qos: .userInitiated).async {
          self?.answer(invoke) {
            guard let data = image.jpegData(compressionQuality: 0.9) else {
              throw Problem("The photo could not be kept.")
            }
            let stamp = DateFormatter()
            stamp.locale = Locale(identifier: "en_US_POSIX")
            stamp.dateFormat = "yyyy-MM-dd HH.mm.ss"
            let name = "Photo \(stamp.string(from: Date())).jpg"
            let path = try Outgoing.newFolder(in: folder) + "/" + name
            try data.write(to: URL(fileURLWithPath: path), options: .atomic)
            return ["path": path, "name": name]
          }
        }
      }
      camera.delegate = answer
      if self.present(camera, for: invoke) {
        self.camera = answer
      }
    }
  }

  /// iOS opened the app with files or with one of Shodh's calendar links
  /// (the app's Rust side passes them on, with the folder for the copies).
  @objc public func deliverOpened(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(OpenedArgs.self)
    arrived(Outgoing.opened(args.urls, into: args.folder))
    invoke.resolve()
  }

  @objc public func takeIncoming(_ invoke: Invoke) {
    invoke.resolve(incoming.take() ?? ["kind": "none"])
  }

  /// Keep Shodh's folder of sealed files out of iCloud and computer backups.
  @objc public func keepOnPhone(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(FolderArgs.self)
    do {
      try FileManager.default.createDirectory(atPath: args.folder, withIntermediateDirectories: true)
      var folder = URL(fileURLWithPath: args.folder, isDirectory: true)
      var values = URLResourceValues()
      values.isExcludedFromBackup = true
      try folder.setResourceValues(values)
      invoke.resolve()
    } catch {
      invoke.reject(error.localizedDescription)
    }
  }

  // ── Voice ───────────────────────────────────────────────────────

  @objc public func voiceListen(_ invoke: Invoke) {
    voice.listen { text, error in
      if let error = error {
        invoke.reject(error)
      } else {
        invoke.resolve(["text": text ?? ""])
      }
    }
  }

  @objc public func voiceStopListening(_ invoke: Invoke) {
    voice.stopListening()
    invoke.resolve()
  }

  @objc public func voiceSpeak(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(TextArgs.self)
    voice.speak(args.text) { error in
      if let error = error {
        invoke.reject(error)
      } else {
        invoke.resolve()
      }
    }
  }

  @objc public func voiceStop(_ invoke: Invoke) {
    voice.stopSpeaking()
    invoke.resolve()
  }

  // ── Notifications ───────────────────────────────────────────────

  @objc public func notificationsAccess(_ invoke: Invoke) {
    notifier.access { granted in
      invoke.resolve(["granted": granted])
    }
  }

  @objc public func notify(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(KindArgs.self)
    notifier.post(args.kind) { error in
      if let error = error {
        invoke.reject(error)
      } else {
        invoke.resolve()
      }
    }
  }

  @objc public func startConnection(_ invoke: Invoke) {
    notifier.stay(true)
    invoke.resolve()
  }

  @objc public func stopConnection(_ invoke: Invoke) {
    notifier.stay(false)
    invoke.resolve()
  }

  @objc public func scheduleBriefs(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(BriefsArgs.self)
    notifier.schedule(args.briefs.map { ($0.eventId, $0.atMs) }) { error in
      if let error = error {
        invoke.reject(error)
      } else {
        invoke.resolve()
      }
    }
  }

  // ── The phone's own calendar ────────────────────────────────────

  @objc public func calendarAccess(_ invoke: Invoke) {
    calendar.access { [weak self] granted in
      if granted { self?.watchCalendar() }
      invoke.resolve(["granted": granted])
    }
  }

  private func needCalendar(_ invoke: Invoke) -> Bool {
    if PhoneCalendar.granted() { return true }
    invoke.reject("Shodh may not use this phone's calendar. Allow it in Settings, Shodh, Calendars.")
    return false
  }

  @objc public func calendarList(_ invoke: Invoke) {
    guard needCalendar(invoke) else { return }
    invoke.resolve(["calendars": calendar.list()])
  }

  @objc public func calendarEnsureShodh(_ invoke: Invoke) {
    guard needCalendar(invoke) else { return }
    answer(invoke) { ["id": try calendar.ensureShodh()] }
  }

  @objc public func calendarEvents(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(EventsArgs.self)
    guard needCalendar(invoke) else { return }
    invoke.resolve(["events": calendar.events(calendars: args.calendarIds, fromMs: args.fromMs, toMs: args.toMs)])
  }

  @objc public func calendarInsert(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(InsertArgs.self)
    guard needCalendar(invoke) else { return }
    answer(invoke) { ["eventId": try calendar.insert(calendar: args.calendarId, fields: args.fields)] }
  }

  @objc public func calendarUpdate(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(UpdateArgs.self)
    guard needCalendar(invoke) else { return }
    answer(invoke) {
      try calendar.update(calendar: args.calendarId, event: args.eventId, fields: args.fields)
      return [:]
    }
  }

  @objc public func calendarDelete(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(DeleteArgs.self)
    guard needCalendar(invoke) else { return }
    answer(invoke) {
      try calendar.delete(calendar: args.calendarId, event: args.eventId)
      return [:]
    }
  }
}

@_cdecl("init_plugin_shodh_ios")
public func initPlugin() -> Plugin {
  return ShodhPlugin()
}
