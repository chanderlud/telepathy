# Local Desktop Distribution Builds

Build installers from repository root with scripts for native operating system only. Do not run macOS script on Linux or Windows, Windows script on macOS or Linux, or Linux script on macOS or Windows. Cross-compilation does not replace platform packaging tools.

All builds need Flutter `3.35.7`, Rust stable through `rustup`, and project dependencies:

```sh
flutter pub get
```

Local artifacts are for development and controlled testing. They have no GitHub Release provenance attestation and no code signing. Before trusting one, verify repository source, checked-out commit, build script, and output location yourself. For distributable release assets, use GitHub Releases and follow [release attestation verification](../README.md#verifying-release-artifacts).

## macOS

Run on macOS only. Install Xcode and its macOS SDKs, then select or accept Xcode's license if prompted. Script uses Xcode build tools and `hdiutil`.

```sh
./scripts/build-macos-installer.sh
```

Expected output:

```text
telepathy-macos-universal-unsigned.dmg
```

DMG contains universal Apple Silicon and Intel app bundle. It is unsigned and not notarized.

### Opening verified local build

1. Mount `telepathy-macos-universal-unsigned.dmg` in Finder.
2. Move `Telepathy.app` to `/Applications`.
3. Verify source checkout, commit, and build output before opening app.
4. In Finder, Control-click `Telepathy.app`, choose **Open**, then confirm **Open**. If macOS instead shows app-specific security prompt, open **System Settings > Privacy & Security** and choose **Open Anyway** for this verified app.

This exception bypasses developer-identity check only for that verified app. It does not sign or notarize app, and it does not disable Gatekeeper globally. Do not use `xattr` to remove quarantine metadata.

## Windows

Run on Windows only. Install Flutter, Rust, Visual Studio with Desktop development with C++ workload, and Inno Setup 6 at its standard compiler location: `C:\Program Files (x86)\Inno Setup 6\ISCC.exe`. Script invokes that path directly; it does not search `PATH` for `ISCC`.

From PowerShell:

```powershell
.\scripts\build-windows-installer.ps1
```

Expected output:

```text
windows\Output\telepathy_installer.exe
```

Script builds Windows release bundle, then packages it with Inno Setup. Installer is unsigned. Windows SmartScreen can warn because publisher identity is not established. Continue only after verifying source checkout, commit, script, and produced file. A local build has no release provenance attestation or code-signing trust signal.

## Linux

Run on Linux only. Install native Flutter dependencies for distribution: Clang, CMake, Ninja, GTK 3, ALSA, LZMA, libsecret, `rpm`, `patchelf`, and `flutter_distributor`. Debian-based systems also need Debian packaging tools; RPM-based systems need RPM build tools.

```sh
dart pub global activate flutter_distributor
./scripts/build-linux-installers.sh
```

Expected outputs:

```text
dist/<release-output>/telepathy-<version>-linux.deb
dist/<release-output>/telepathy-<version>-linux.rpm
```

`<version>` comes from project package metadata. Packages target host architecture. Build on intended Linux architecture, such as x86_64 or aarch64. Do not treat package built on one architecture as installable on another.

Install verified local package with platform package manager:

```sh
sudo apt install ./dist/<release-output>/telepathy-<version>-linux.deb
sudo dnf install ./dist/<release-output>/telepathy-<version>-linux.rpm
```

These packages are unsigned local artifacts. Verify source, commit, build script, and output before installation. They do not carry GitHub Release provenance attestations.
