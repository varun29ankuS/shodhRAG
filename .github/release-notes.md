## Downloads

| System | File |
| --- | --- |
| Windows 10 or 11 (64-bit) | `shodh-rag-app_<version>_x64-setup.exe` |
| macOS on Apple Silicon (M1 and later) | `shodh-rag-app_<version>_aarch64.dmg` |
| macOS on Intel | `shodh-rag-app_<version>_x64.dmg` |

The installer does not contain the search models (about 620 MB), the OCR model (about 31 MB) or the agent runtime. Shodh downloads each one, checks its SHA-256 hash, and keeps it the first time you need it.

## These builds are not signed

Neither the Windows installer nor the macOS app is signed with a publisher certificate yet, so your system warns you before the first launch. The steps below get past the warning once; later launches open normally.

### Windows: "Windows protected your PC"

1. If your browser says the file "isn't commonly downloaded", open the download list, choose **...** (More actions) next to the file, then **Keep**, then **Show more** and **Keep anyway**.
2. Run `shodh-rag-app_<version>_x64-setup.exe`.
3. When Microsoft Defender SmartScreen shows "Windows protected your PC", click **More info**, then **Run anyway**.

The installer installs for your user account only and does not ask for administrator rights.

### macOS: "cannot be opened" or "is damaged"

1. Open the `.dmg` and drag **shodh-rag-app** into **Applications**.
2. Open it once from Applications. macOS refuses it ("cannot be opened because Apple cannot check it for malicious software", "Apple could not verify ...", or "is damaged and can't be opened"). Click **Done** or **Cancel**, not **Move to Trash**.
3. Then, depending on your macOS version:
   - **macOS 15 (Sequoia) and later:** open **System Settings → Privacy & Security**, scroll to **Security**, click **Open Anyway** next to the message about shodh-rag-app, and confirm with your password.
   - **macOS 14 (Sonoma) and earlier:** in Finder, Control-click (or right-click) **shodh-rag-app** in Applications, choose **Open**, then **Open** again in the dialog.

If macOS says the app "is damaged", or the steps above do not offer **Open Anyway**, remove the download quarantine flag in Terminal and open the app again:

```sh
xattr -dr com.apple.quarantine /Applications/shodh-rag-app.app
```
