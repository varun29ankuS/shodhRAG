package com.shodhrag.android

import android.app.Activity
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import android.speech.tts.TextToSpeech
import android.speech.tts.UtteranceProgressListener
import java.util.Locale
import java.util.UUID

/**
 * Voice: the phone's own speech recognition (on the device when the phone
 * has it) to ask, and its text to speech to read an answer aloud. Nothing is
 * recorded or kept; the microphone is used only while the user holds Talk.
 * Android requires both on the main thread.
 */
internal class Voice(private val activity: Activity) {
    private var recognizer: SpeechRecognizer? = null
    private var tts: TextToSpeech? = null
    private var ttsReady = false

    fun listen(done: (String?, String?) -> Unit) {
        activity.runOnUiThread {
            if (!SpeechRecognizer.isRecognitionAvailable(activity)) {
                done(null, "This phone has no speech recognition.")
                return@runOnUiThread
            }
            recognizer?.destroy()
            val onDevice = Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
                SpeechRecognizer.isOnDeviceRecognitionAvailable(activity)
            val created = if (onDevice) SpeechRecognizer.createOnDeviceSpeechRecognizer(activity) else SpeechRecognizer.createSpeechRecognizer(activity)
            recognizer = created
            var answered = false
            val answer = { text: String?, error: String? ->
                if (!answered) {
                    answered = true
                    done(text, error)
                    created.destroy()
                    if (recognizer === created) recognizer = null
                }
            }
            created.setRecognitionListener(object : RecognitionListener {
                override fun onResults(results: Bundle?) {
                    val heard = results?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull()
                    answer(heard?.trim().orEmpty(), null)
                }

                override fun onError(error: Int) {
                    val words = when (error) {
                        SpeechRecognizer.ERROR_NO_MATCH, SpeechRecognizer.ERROR_SPEECH_TIMEOUT -> "Shodh did not catch that. Try again."
                        SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS -> "Shodh needs the microphone to listen."
                        SpeechRecognizer.ERROR_NETWORK, SpeechRecognizer.ERROR_NETWORK_TIMEOUT -> "Speech recognition needs the network on this phone."
                        else -> "Speech recognition stopped."
                    }
                    answer(null, words)
                }

                override fun onReadyForSpeech(params: Bundle?) {}
                override fun onBeginningOfSpeech() {}
                override fun onRmsChanged(rmsdB: Float) {}
                override fun onBufferReceived(buffer: ByteArray?) {}
                override fun onEndOfSpeech() {}
                override fun onPartialResults(partialResults: Bundle?) {}
                override fun onEvent(eventType: Int, params: Bundle?) {}
            })
            val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH)
                .putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
                .putExtra(RecognizerIntent.EXTRA_PREFER_OFFLINE, true)
                .putExtra(RecognizerIntent.EXTRA_MAX_RESULTS, 1)
            created.startListening(intent)
        }
    }

    fun stopListening() {
        activity.runOnUiThread { recognizer?.stopListening() }
    }

    fun speak(text: String, done: (String?) -> Unit) {
        activity.runOnUiThread {
            val engine = tts
            if (engine != null && ttsReady) {
                say(engine, text, done)
                return@runOnUiThread
            }
            tts = TextToSpeech(activity) { status ->
                activity.runOnUiThread {
                    val ready = tts
                    if (status != TextToSpeech.SUCCESS || ready == null) {
                        done("This phone cannot read aloud.")
                        return@runOnUiThread
                    }
                    ttsReady = true
                    ready.language = Locale.getDefault()
                    say(ready, text, done)
                }
            }
        }
    }

    private fun say(engine: TextToSpeech, text: String, done: (String?) -> Unit) {
        val id = UUID.randomUUID().toString()
        engine.setOnUtteranceProgressListener(object : UtteranceProgressListener() {
            override fun onStart(utteranceId: String?) {}
            override fun onDone(utteranceId: String?) {
                if (utteranceId == id) done(null)
            }

            @Deprecated("Deprecated in Java")
            override fun onError(utteranceId: String?) {
                if (utteranceId == id) done("Reading aloud stopped.")
            }
        })
        // Long answers are read in parts the engine accepts.
        val parts = text.chunked(TextToSpeech.getMaxSpeechInputLength() - 1)
        parts.forEachIndexed { i, part ->
            val mode = if (i == 0) TextToSpeech.QUEUE_FLUSH else TextToSpeech.QUEUE_ADD
            engine.speak(part, mode, null, if (i == parts.lastIndex) id else "$id-$i")
        }
    }

    fun stopSpeaking() {
        activity.runOnUiThread { tts?.stop() }
    }
}
