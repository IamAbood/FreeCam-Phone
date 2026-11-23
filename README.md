# FreeCam Phone

Turn an Android phone into a webcam and microphone for Windows. Open source (MIT), with no accounts, no cloud and no telemetry. Video stays on your LAN or USB cable.

- **FreeCam Phone Camera** shows up as a webcam in the Windows Camera app, browsers, Zoom/Teams and OBS.
  - Windows 11: native Media Foundation virtual camera.
  - Windows 10 / legacy apps: DirectShow filter (built on [softcam](https://github.com/tshino/softcam), MIT).
- Connect over **Wi-Fi** with automatic discovery (mDNS), or over **USB** via `adb`. USB is preferred when both work, and the app reconnects automatically.
- Remote controls from the PC: switch camera, 480p/720p/1080p, 30/60 fps, torch, tap-to-focus, exposure, zoom, mic on/off, plus mirror.
- **Phone microphone** on the PC through a virtual audio cable (see below).
- Latency and fps are shown live in the Windows app.

## Install

1. **Phone:** install `FreeCamPhone-x.y.z.apk` from the [Releases](../../releases) page and allow camera and microphone access. Keep the app open in the foreground. Hold the phone in landscape.
2. **PC:** run `FreeCamPhone-Setup-x.y.z.exe`. It needs admin rights because it registers the virtual camera DLLs. Then start **FreeCam Phone**.
3. **Wi-Fi:** keep the phone and PC on the same network. The phone appears in the device list within a few seconds.
4. **USB (optional, lower latency):** enable *Developer options → USB debugging* on the phone and install Google's [platform-tools](https://developer.android.com/tools/releases/platform-tools) (`adb`) on the PC. The app finds `adb` on `PATH`, in `%ANDROID_HOME%` or `%LOCALAPPDATA%\Android\Sdk`, or at the path you set in Settings.

Choose **FreeCam Phone Camera** as the camera in your app. While no phone is connected, the camera shows a "FreeCam Phone — waiting for phone" card instead of failing.

### Microphone

Windows only lets kernel drivers create microphone devices. FreeCam Phone therefore plays the phone mic into a virtual audio cable:

1. Install [VB-CABLE](https://vb-audio.com/Cable/) (free; it's a separate driver, not part of this project).
2. In FreeCam Phone → Settings → **Phone audio to**, pick **CABLE Input** (the default).
3. In Zoom/Teams/Discord, select **CABLE Output** as the microphone.

A native "FreeCam Phone Microphone" device would need our own signed audio driver. That is possible future work and is not part of v1.

## Settings

Settings are saved in `%APPDATA%\FreeCamPhone\settings.toml`.

| Setting | Meaning |
|---|---|
| Virtual camera | `Auto` (MF on Win 11, DirectShow otherwise), `Media Foundation`, `DirectShow`, or `Both` |
| Mirror video | Flips the image horizontally for every app |
| Start with Windows | Starts minimised to the tray (HKCU Run key) |
| adb path | Leave empty to auto-detect |

Closing the window hides it to the tray. Use the tray's **Quit** to exit.

## Troubleshooting

| Symptom | Fix |
|---|---|
| Phone not listed on Wi-Fi | Same subnet? Some routers block multicast ("AP/client isolation"); use USB instead. Allow `freecam-phone.exe` through Windows Firewall (private networks). |
| USB device not listed | Run `adb devices`. The phone must show `device`, not `unauthorized` (accept the prompt on the phone). Set the adb path in Settings if it isn't found. |
| Camera missing in apps | Reinstall (registers the DLLs) or run `scripts\register-dev.ps1` as admin. Restart the app that uses the camera. On Windows 11 the MF camera only exists while FreeCam Phone is running. |
| "DirectShow camera busy" | Another softcam-based app is running. Close it and restart FreeCam Phone. |
| Black or placeholder image | Make sure the phone app is in the foreground with the screen on. |
| Image upside-down | Rotate the phone 180° (the app streams in landscape). |
| High latency | Use USB, 720p, or 30 fps. The phone's 5 GHz Wi-Fi helps. |
| No mic audio | Is VB-CABLE installed? Check **Phone audio to** and the mic toggle, and select *CABLE Output* in your app. |

## How it works

```
Android (Kotlin)                          Windows (Rust)
Camera2 ─► MediaCodec H.264 ─┐            freecam-phone.exe
AudioRecord 48k mono ────────┤  TCP       ├─ mDNS browse / adb forward, auto-reconnect
                             ▼  (LAN/USB) ├─ MF H.264 decoder MFT → NV12
              libphone_core.so (Rust) ───►├─ preview (egui) + tray
              TCP server, mDNS, protocol  ├─ framebus (shared memory) ─► freecam_vcam_mf.dll (Frame Server, Win 11)
                                          ├─ softcam sender ──────────► freecam_dshow.dll (DirectShow apps)
                                          └─ WASAPI ─► CABLE Input
```

The wire protocol (`crates/protocol`) is one TCP connection carrying `[u8 type][u32 len][payload]` packets: Hello, VideoConfig, VideoFrame, AudioConfig, AudioFrame, Control, Stats, Ping/Pong. The phone is the server on port 47800 and advertises `_freecamphone._tcp`.

| Path | What |
|---|---|
| `crates/protocol` | Wire format plus the `freecam-dump` debug tool (writes `out.h264` / `out.pcm`) |
| `crates/phone-core` | Android cdylib: JNI, tokio TCP server, mDNS |
| `crates/receiver` | Windows app (`freecam-phone.exe`) |
| `crates/framebus` | Cross-process NV12 ring buffer, readable by the Frame Server (LOCAL SERVICE) |
| `crates/vcam-mf` | Media Foundation virtual camera media source (COM DLL) |
| `crates/vcam-dshow` | DirectShow filter + sender via softcam (git submodule) |
| `android/` | Kotlin app |

## Building from source

Requirements:
- Rust (stable, MSVC)
- Visual Studio Build Tools with the C++ workload and Windows SDK
- Android SDK + NDK and JDK 17+
- `cargo install cargo-ndk`
- `rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android`
- Inno Setup (for the installer)

```powershell
git clone --recursive https://github.com/IamAbood/FreeCam-Phone
cargo test --workspace                     # unit tests
cd android; .\gradlew assembleDebug        # APK (builds the Rust core via cargo-ndk)
.\scripts\build-release.ps1 -Version 0.1.0 # APK + installer into .\dist
```

For local testing without the installer, build release binaries and run `scripts\register-dev.ps1` from an elevated PowerShell. Use `-Unregister` to undo.

Debug the phone stream without the GUI:

```
adb forward tcp:47801 tcp:47800
cargo run -p protocol --bin freecam-dump -- 127.0.0.1 47801 10
ffplay out.h264
```

CI (`.github/workflows/ci.yml`) runs fmt, clippy and tests on Windows and builds the APK on every push. Pushing a `v*` tag runs `release.yml`, which builds the installer and APK and attaches both to a GitHub Release.

## Known limitations (v1)

- The phone app must stay in the foreground (Android blocks background camera use).
- The microphone goes through a third-party virtual cable, not a native device.
- Video decodes on the CPU (Media Foundation's software path, multi-threaded).
- The release APK is signed with the debug key. Add your own keystore before publishing to a store.

## License

MIT. Bundles softcam (MIT, © tshino) for the DirectShow filter.
