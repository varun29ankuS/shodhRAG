import Foundation
import Tauri
import UIKit

/// What went wrong, in words the window shows as they are.
struct Problem: LocalizedError {
  let words: String

  init(_ words: String) {
    self.words = words
  }

  var errorDescription: String? { words }
}

/// Files going to the computer: what the user picked, photographed or opened
/// with Shodh is copied at once into the app's own cache (`outgoing/`, the
/// folder the app's Rust side names and later checks), because iOS's
/// permission to read a picked or shared file ends with the picker or the
/// app. The copy is what the outbox keeps and sends; it is removed once the
/// computer has it.
enum Outgoing {
  /// The largest file copied (the computer's largest limit, for PDFs).
  static let maxBytes: UInt64 = 512 * 1024 * 1024

  /// The most files taken from one share or one pick (the Send sheet's).
  static let maxFiles = 20

  /// The "Open in Shodh" link of an event Shodh put in the phone's calendar.
  static let eventLink = "shodh://event/"

  /// A plain file name: no folders, no control characters, never empty.
  static func plainName(_ raw: String?, fallback: String) -> String {
    var name = raw ?? ""
    if let slash = name.lastIndex(where: { $0 == "/" || $0 == "\\" }) {
      name = String(name[name.index(after: slash)...])
    }
    var kept = String.UnicodeScalarView()
    for scalar in name.unicodeScalars where scalar.value >= 0x20 && scalar != ":" {
      kept.append(scalar)
    }
    name = String(kept).trimmingCharacters(in: .whitespaces)
    name = name.trimmingCharacters(in: CharacterSet(charactersIn: "."))
    return name.isEmpty ? fallback : String(name.prefix(200))
  }

  /// A new folder for one file, so two files of one name stay apart. The
  /// path is the given folder's own text with a name added, so the app's
  /// Rust side finds it under the folder it named.
  static func newFolder(in outgoing: String) throws -> String {
    let folder = outgoing + "/" + UUID().uuidString.lowercased()
    try FileManager.default.createDirectory(atPath: folder, withIntermediateDirectories: true)
    return folder
  }

  /// Copy the file at `url` into a new folder of `outgoing`; its path and its name.
  static func copy(_ url: URL, into outgoing: String) throws -> JsonObject {
    // A file picked or shared from another app's storage is readable only
    // inside this scope; for the app's own files the call changes nothing.
    let scoped = url.startAccessingSecurityScopedResource()
    defer {
      if scoped { url.stopAccessingSecurityScopedResource() }
    }
    let name = plainName(url.lastPathComponent, fallback: "file")
    let values = try? url.resourceValues(forKeys: [.isDirectoryKey, .fileSizeKey])
    if values?.isDirectory == true {
      throw Problem("\(name) is a folder. Shodh sends files.")
    }
    if let size = values?.fileSize, UInt64(size) > maxBytes {
      throw Problem("\(name) is larger than Shodh can read")
    }
    let folder = try newFolder(in: outgoing)
    let target = folder + "/" + name
    do {
      try FileManager.default.copyItem(atPath: url.path, toPath: target)
    } catch {
      try? FileManager.default.removeItem(atPath: folder)
      throw Problem("\(name) could not be read")
    }
    // The size iOS reported is not the last word (some providers give none).
    let attributes = try? FileManager.default.attributesOfItem(atPath: target)
    let copied = (attributes?[.size] as? NSNumber)?.uint64Value ?? 0
    if copied > maxBytes {
      try? FileManager.default.removeItem(atPath: folder)
      throw Problem("\(name) is larger than Shodh can read")
    }
    return ["path": target, "name": name]
  }

  /// Copies of every file of `urls` (at most `maxFiles`), as the window's `files` list.
  static func copies(_ urls: [URL], into outgoing: String) throws -> [JsonObject] {
    var files: [JsonObject] = []
    for url in urls.prefix(maxFiles) {
      files.append(try copy(url, into: outgoing))
    }
    return files
  }

  /// iOS's own copy of a file handed to Shodh (in the app's `Documents/Inbox`
  /// when it was opened with Shodh, in the app's temporary folder when it
  /// was picked) is not needed once Shodh has made its copy. Nothing
  /// outside those two folders is ever removed.
  static func removeSystemCopy(_ url: URL) {
    let path = url.resolvingSymlinksInPath().path
    var folders = [URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)]
    if let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask).first {
      folders.append(documents.appendingPathComponent("Inbox", isDirectory: true))
    }
    for folder in folders {
      var prefix = folder.resolvingSymlinksInPath().path
      if !prefix.hasSuffix("/") { prefix += "/" }
      if path.hasPrefix(prefix) {
        try? FileManager.default.removeItem(atPath: path)
        return
      }
    }
  }

  /// What iOS opened the app with, as the window reads it (`Incoming` in
  /// mobile/src/lib/device.ts): one of Shodh's calendar events, or files.
  static func opened(_ links: [String], into outgoing: String) -> JsonObject {
    var files: [URL] = []
    for link in links {
      if link.hasPrefix(eventLink) {
        let id = String(link.dropFirst(eventLink.count))
        let plain = !id.isEmpty && id.count <= 100
          && id.unicodeScalars.allSatisfy { CharacterSet.alphanumerics.contains($0) || $0 == "-" }
        if plain {
          return ["kind": "event", "id": id]
        }
        return ["kind": "error", "message": "This event is not one of Shodh's."]
      }
      if let url = URL(string: link), url.isFileURL {
        files.append(url)
      }
    }
    if files.isEmpty {
      return ["kind": "error", "message": "Nothing Shodh can read was shared."]
    }
    do {
      let copied = try copies(files, into: outgoing)
      for url in files { removeSystemCopy(url) }
      return ["kind": "files", "files": copied]
    } catch {
      for url in files { removeSystemCopy(url) }
      return ["kind": "error", "message": error.localizedDescription]
    }
  }
}

/// What arrived and is waiting for the window to take it.
final class Incoming {
  private let lock = NSLock()
  private var pending: JsonObject?

  func deliver(_ payload: JsonObject) {
    lock.lock()
    pending = payload
    lock.unlock()
  }

  /// What arrived, once.
  func take() -> JsonObject? {
    lock.lock()
    defer { lock.unlock() }
    let payload = pending
    pending = nil
    return payload
  }
}

/// The file picker's answer: the chosen files, or none when the user went
/// back, by Cancel or by swiping the sheet away. It answers once.
final class PickerAnswer: NSObject, UIDocumentPickerDelegate, UIAdaptivePresentationControllerDelegate {
  private var done: (([URL]) -> Void)?

  init(_ done: @escaping ([URL]) -> Void) {
    self.done = done
  }

  private func answer(_ urls: [URL]) {
    let done = self.done
    self.done = nil
    done?(urls)
  }

  func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
    answer(urls)
  }

  func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
    answer([])
  }

  func presentationControllerDidDismiss(_ presentationController: UIPresentationController) {
    answer([])
  }
}

/// The camera's answer: the photo taken, or none when the user went back.
/// It answers once.
final class CameraAnswer: NSObject, UIImagePickerControllerDelegate, UINavigationControllerDelegate {
  private var done: ((UIImage?) -> Void)?

  init(_ done: @escaping (UIImage?) -> Void) {
    self.done = done
  }

  private func answer(_ image: UIImage?, from picker: UIImagePickerController) {
    let done = self.done
    self.done = nil
    picker.dismiss(animated: true) {
      done?(image)
    }
  }

  func imagePickerController(
    _ picker: UIImagePickerController,
    didFinishPickingMediaWithInfo info: [UIImagePickerController.InfoKey: Any]
  ) {
    answer(info[.originalImage] as? UIImage, from: picker)
  }

  func imagePickerControllerDidCancel(_ picker: UIImagePickerController) {
    answer(nil, from: picker)
  }
}
