import AVFoundation
import Foundation
import Speech

/// Voice: the phone's own speech recognition (on the device when the phone
/// has it) to ask, and its text to speech to read an answer aloud. Nothing is
/// recorded or kept; the microphone is used only while Shodh listens. All of
/// it runs on the main thread.
final class Voice: NSObject, AVSpeechSynthesizerDelegate {
  /// Quiet for this long after the last word, and the user has finished.
  private static let quiet: TimeInterval = 1.6

  /// Nothing heard for this long from the start, and Shodh stops listening.
  private static let nothingHeard: TimeInterval = 8

  private let engine = AVAudioEngine()
  private var request: SFSpeechAudioBufferRecognitionRequest?
  private var task: SFSpeechRecognitionTask?
  private var heard = ""
  private var silence: Timer?
  private var answer: ((String?, String?) -> Void)?

  private let synthesizer = AVSpeechSynthesizer()
  /// What is being read, and who waits for it to end.
  private var reading: AVSpeechUtterance?
  private var spoken: ((String?) -> Void)?

  override init() {
    super.init()
    synthesizer.delegate = self
  }

  // ── Listening ───────────────────────────────────────────────────

  /// Listen once: what the user said, or why not, in words the window shows.
  func listen(_ done: @escaping (String?, String?) -> Void) {
    SFSpeechRecognizer.requestAuthorization { status in
      guard status == .authorized else {
        done(nil, "Shodh needs speech recognition to listen. Allow it in Settings, Shodh.")
        return
      }
      AVAudioSession.sharedInstance().requestRecordPermission { granted in
        DispatchQueue.main.async {
          guard granted else {
            done(nil, "Shodh needs the microphone to listen. Allow it in Settings, Shodh.")
            return
          }
          self.start(done)
        }
      }
    }
  }

  private func start(_ done: @escaping (String?, String?) -> Void) {
    // A listen still open is answered with what it heard before the new one starts.
    finish(heard, nil)
    guard let recognizer = SFSpeechRecognizer(), recognizer.isAvailable else {
      done(nil, "This iPhone has no speech recognition right now.")
      return
    }
    let request = SFSpeechAudioBufferRecognitionRequest()
    request.shouldReportPartialResults = true
    if recognizer.supportsOnDeviceRecognition {
      request.requiresOnDeviceRecognition = true
    }
    do {
      let session = AVAudioSession.sharedInstance()
      try session.setCategory(.playAndRecord, mode: .measurement, options: [.duckOthers, .defaultToSpeaker])
      try session.setActive(true, options: .notifyOthersOnDeactivation)
      let input = engine.inputNode
      let format = input.outputFormat(forBus: 0)
      guard format.sampleRate > 0, format.channelCount > 0 else {
        done(nil, "This iPhone's microphone is not available.")
        return
      }
      input.removeTap(onBus: 0)
      input.installTap(onBus: 0, bufferSize: 1024, format: format) { buffer, _ in
        request.append(buffer)
      }
      engine.prepare()
      try engine.start()
    } catch {
      engine.inputNode.removeTap(onBus: 0)
      done(nil, "Shodh could not use the microphone.")
      return
    }
    self.request = request
    self.answer = done
    self.heard = ""
    waitForQuiet(Voice.nothingHeard)
    task = recognizer.recognitionTask(with: request) { [weak self] result, error in
      DispatchQueue.main.async {
        guard let self = self, self.request === request else { return }
        if let result = result {
          self.heard = result.bestTranscription.formattedString
          if result.isFinal {
            self.finish(self.heard, nil)
          } else {
            self.waitForQuiet(Voice.quiet)
          }
        } else if error != nil {
          if self.heard.isEmpty {
            self.finish(nil, "Shodh did not catch that. Try again.")
          } else {
            self.finish(self.heard, nil)
          }
        }
      }
    }
  }

  /// End the listen after `seconds` without a new word.
  private func waitForQuiet(_ seconds: TimeInterval) {
    silence?.invalidate()
    silence = Timer.scheduledTimer(withTimeInterval: seconds, repeats: false) { [weak self] _ in
      guard let self = self else { return }
      if self.heard.isEmpty {
        self.finish(nil, "Shodh did not catch that. Try again.")
      } else {
        self.finish(self.heard, nil)
      }
    }
  }

  /// The user tapped Stop: what was heard so far is the answer.
  func stopListening() {
    DispatchQueue.main.async {
      self.finish(self.heard, nil)
    }
  }

  /// Stop the microphone and answer the open listen, once.
  private func finish(_ text: String?, _ error: String?) {
    guard let answer = answer else { return }
    self.answer = nil
    silence?.invalidate()
    silence = nil
    if engine.isRunning { engine.stop() }
    engine.inputNode.removeTap(onBus: 0)
    request?.endAudio()
    task?.cancel()
    request = nil
    task = nil
    heard = ""
    try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    answer(text?.trimmingCharacters(in: .whitespacesAndNewlines), error)
  }

  // ── Reading aloud ───────────────────────────────────────────────

  /// Read `text` aloud; `done` when it was read (or stopped), with why not if it could not be.
  func speak(_ text: String, _ done: @escaping (String?) -> Void) {
    DispatchQueue.main.async {
      // A reading still going is ended first, and its caller answered here:
      // the synthesizer reports its end later, when `reading` is already
      // the new one (see `ended`).
      let waiting = self.spoken
      self.spoken = nil
      self.reading = nil
      if self.synthesizer.isSpeaking {
        self.synthesizer.stopSpeaking(at: .immediate)
      }
      waiting?(nil)
      do {
        let session = AVAudioSession.sharedInstance()
        try session.setCategory(.playback, mode: .spokenAudio, options: [.duckOthers])
        try session.setActive(true, options: .notifyOthersOnDeactivation)
      } catch {
        done("This iPhone cannot read aloud right now.")
        return
      }
      let utterance = AVSpeechUtterance(string: text)
      utterance.voice = AVSpeechSynthesisVoice(language: AVSpeechSynthesisVoice.currentLanguageCode())
      self.reading = utterance
      self.spoken = done
      self.synthesizer.speak(utterance)
    }
  }

  func stopSpeaking() {
    DispatchQueue.main.async {
      if self.synthesizer.isSpeaking {
        self.synthesizer.stopSpeaking(at: .immediate)
      }
    }
  }

  /// `utterance` was read to its end or stopped. Only the one being read
  /// now answers: an earlier one's end arrives after the next has started.
  private func ended(_ utterance: AVSpeechUtterance) {
    guard utterance === reading else { return }
    let done = spoken
    spoken = nil
    reading = nil
    try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    done?(nil)
  }

  func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didFinish utterance: AVSpeechUtterance) {
    ended(utterance)
  }

  func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didCancel utterance: AVSpeechUtterance) {
    ended(utterance)
  }
}
