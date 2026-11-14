MNX DATAFORGE  -  Android AI dataset engineering studio
=======================================================

Architecture (decision recorded in journal/phase1-environment.txt):
  - UI .............. Kotlin + Jetpack Compose (foundation only, no Material), custom liquid-glass design system
  - Engine .......... Rust (zero dependencies), called through JNI: JSON/JSONL/CSV, validation, dedup,
                      splits, leakage check, export profiles, statistics
  - Storage ......... Android SQLite (WAL, FTS4 unicode61), content-addressed blobs for history and versions
  - Forbidden ....... Google Play Services, WebView, any permission (manifest declares none)
  - Target .......... Android 15+ (minSdk 35), arm64-v8a only

HOW TO GET THE APK (GitHub Actions, nothing to install locally)
  1. Create a new GitHub repository and upload the contents of this archive (keep the folder layout).
  2. Open the repository's "Actions" tab. The workflow "Build APK" runs on every push; you can also press
     "Run workflow".
  3. When it finishes, open the run and download the artifact "MNX-DataForge-apk" (contains app-release.apk).
  4. Copy the APK to the phone and install it (allow "install unknown apps" for the file manager).

  The workflow does: Rust tests on the host -> cargo-ndk builds libdataforge_jni.so for arm64 ->
  Gradle assembleRelease. The release APK is signed with the DEBUG key so it installs directly. Replace that
  with your own keystore before publishing anywhere.

BUILD LOCALLY (needs JDK 17, Android SDK 35 + build-tools, NDK, rustup target aarch64-linux-android)
  cargo install cargo-ndk --locked
  cargo ndk -t arm64-v8a -o app/src/main/jniLibs build --release -p dataforge-jni
  gradle :app:assembleRelease          (Gradle 8.10+)

RUN THE ENGINE TESTS (any machine with Rust)
  cargo test -p dataforge-core
  cargo run --release --example bench -p dataforge-core -- 50000

LAYOUT
  core/            Rust engine library (pure std)
  jni/             Rust JNI bridge (cdylib)  ->  com.monex.dataforge.core.NativeCore.call(op, json)
  app/             Android app (Kotlin)
    data/Db.kt       schema, repository, snapshots, diff, backup/restore
    ops/Ops.kt       import, export, validation runs, split planning, template expansion
    ui/              design system (Theme.kt), screens
  docs/STATUS.txt  exact state of every requirement: what is verified and what is not
  journal/         decision and build log

IMPORTANT: read docs/STATUS.txt first. The Kotlin app has NOT been compiled or run on a device yet.
