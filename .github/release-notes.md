## Before you update from 0.0.10: the library upgrade is one-way

The first time 0.0.11 opens your library, it moves it to a new storage format. After that, 0.0.10 cannot open it again, so going back to 0.0.10 means indexing your files again. If you are not sure you will stay on 0.0.11, back up your library first: quit Shodh and copy its data folder (`%APPDATA%\com.shodh.rag-app` on Windows, `~/Library/Application Support/com.shodh.rag-app` on macOS) somewhere safe.

After the update, Shodh reads some files again, once, in the background:

- **Read and indexed again:** Word, PowerPoint, plain text and code files, web pages, emails, e-books, notebooks and spreadsheets. Their passages are now cut to fit what the search model reads, so the end of a long code or data passage is no longer left out of search. Code and data files make more passages than before, so this can take a while on a large library; search keeps working meanwhile.
- **Kept as they are:** Markdown, LaTeX, images and most PDFs. They are not read again, and scans are not run through OCR again. A PDF with form fields, or one Shodh could not split into sections, is read again like the files above (a scan whose reading was not kept keeps its passages instead).
- **Removed:** files that may hold passwords or keys (such as `.env` files and key files) that an earlier version indexed. Shodh no longer indexes them at all.
- A library of more than 20,000 passages gets a new search index once, so large libraries search faster.

Your conversations and memories are kept.

Two other things change on the update:

- **Pick a mode for each workspace.** The approval level a workspace had in 0.0.10 (such as Trusted) is replaced by the modes Ask, Accept edits, Plan and Auto, and the saved level is ignored: every workspace starts in **Ask**, which asks before every change, until you choose its mode. The commands you allowed stay allowed, as extra commands Auto runs (#121).
- **The preview opens in a window of its own.** A page the assistant builds (Artifacts) now opens in a separate, normal window when you click **Preview**, no longer inside the main window, so it cannot reach the app (#124).

## What's new in 0.0.11

- **Asking works on Windows however long the instructions are.** The assistant's instructions now reach the agent runtime in a file, so a long set of rules and skills no longer stops a question with "The filename or extension is too long" (#179).
- **Your saved rules are not dropped on a busy computer.** When the computer was slow, a question could start without the standing rules you saved in memory. They are now always included (#182).
- **Faster and safer, thanks to John (Portll on GitHub).** Dependencies with security advisories were raised or removed (#164, #172), the library is tidied once per batch of changes instead of after every removal and gets a proper search index past 20,000 passages (#169), passages fit the search model (#170), Core ML on a Mac is faster and loads in the background (#171, #180), and the tests run on macOS (#165). Thank you, John.
- **Code intelligence.** In Code mode the assistant sees a map of your project, checks designs, compresses long command output and shows the tokens it saved (#156, #158, #159). It can also review a change with fresh eyes (#153).
- **Recall.** Shodh can index your own Claude Code and Codex sessions, on this computer, so you and the assistant can search what you worked on before. It is off until you set it up in Library, and secrets in the transcripts are redacted (#161).
- **More file types.** Old Word (`.doc`) and PowerPoint (`.ppt`) files, OpenDocument text and slides, RTF, e-books, emails, notebooks, logs and GIF and WebP images are indexed (#163).
- **Secret protection.** Files that may hold passwords or keys are never indexed, and passwords or keys inside other files are redacted before they are stored (#163).
- **The phone link, desktop side.** Settings has a Phone section to pair a phone over an encrypted link, and conversations are saved by the app itself, so an answer is kept even when no window is open (#173, #174). Shodh for Android is in this release too: see "Android: install the APK" below.
- **Agent modes and more.** Ask, Accept edits, Plan and Auto modes (#121), Creator mode and the Artifacts tab (#122, #124), a GitHub connector with per-repository permissions (#120), a plan checklist and stricter checks before opening links or mentioning people (#166), and the assistant can operate the app where you can see it (#154).
- **Search and speed.** The search model can run on a graphics card, with pause and cancel for indexing (#119), an opt-in faster search model (int8) on processors that run it exactly (#125), better retrieval on research papers (#111) and table rows kept together in PDFs (#112).
- **Reliability.** A backup model for every model call (#108), quieter logs, steadier updates and reconnects (#109), the macOS command sandbox (#115), app commands scoped to what each window may do (#123), smoother tab changes (#113) and fixes from testing (#107, #114, #116, #117, #163).

## Downloads

| System | File |
| --- | --- |
| Windows 10 or 11 (64-bit) | `Shodh_<version>_x64-setup.exe` |
| macOS 13.3 or later on Apple Silicon (M1 and later) | `Shodh_<version>_aarch64.dmg` |
| Android 7 or later, 64-bit phones | `Shodh_<version>_android.apk` |

There is no build for Intel Macs yet.

The installer does not contain the search models (about 620 MB), the OCR model (about 31 MB) or the agent runtime. Shodh downloads each one, checks its SHA-256 hash, and keeps it the first time you need it.

## These builds are not signed

Neither the Windows installer nor the macOS app is signed with a publisher certificate yet, so your system warns you before the first launch. The steps below get past the warning once; later launches open normally.

### Windows: "Windows protected your PC"

1. If your browser says the file "isn't commonly downloaded", open the download list, choose **...** (More actions) next to the file, then **Keep**, then **Show more** and **Keep anyway**.
2. Run `Shodh_<version>_x64-setup.exe`.
3. When Microsoft Defender SmartScreen shows "Windows protected your PC", click **More info**, then **Run anyway**.

The installer installs for your user account only and does not ask for administrator rights.

### macOS: "cannot be opened" or "is damaged"

1. Open the `.dmg` and drag **Shodh** into **Applications**.
2. Open it once from Applications. macOS refuses it ("cannot be opened because Apple cannot check it for malicious software", "Apple could not verify ...", or "is damaged and can't be opened"). Click **Done** or **Cancel**, not **Move to Trash**.
3. Then, depending on your macOS version:
   - **macOS 15 (Sequoia) and later:** open **System Settings → Privacy & Security**, scroll to **Security**, click **Open Anyway** next to the message about Shodh, and confirm with your password.
   - **macOS 14 (Sonoma) and earlier:** in Finder, Control-click (or right-click) **Shodh** in Applications, choose **Open**, then **Open** again in the dialog.

If macOS says the app "is damaged", or the steps above do not offer **Open Anyway**, remove the download quarantine flag in Terminal and open the app again:

```sh
xattr -dr com.apple.quarantine /Applications/Shodh.app
```

## Android: install the APK

Shodh for Android is not in Google Play yet. Install it from this release:

1. On your phone, open this page and download `Shodh_<version>_android.apk`.
2. Open the download. Android asks whether to allow installs from the app you downloaded it with (your browser or Files): choose **Settings**, turn on **Allow from this source**, and go back.
3. Choose **Install**. If Google Play Protect warns about an unknown app, choose **More details**, then **Install anyway**.

You allow that source once; you can turn it off again afterwards. The APK is signed with Shodh's own key, whose SHA-256 certificate fingerprint is `6A:67:45:33:E6:C7:96:B4:C5:92:5E:E3:5C:F2:58:DF:07:E6:BD:A3:24:94:11:10:65:D0:0A:16:B6:06:17:79`. Later versions install over it, and Shodh on your phone offers them when your computer runs a newer Shodh. The phone and the computer must run the same version.
