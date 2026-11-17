MNX DATAFORGE
Professional AI-dataset engineering studio, built natively for Android 15+.

Build, clean, validate, version and export training datasets entirely on your
phone. No cloud, no account, no tracking. Your data never leaves the device
unless you export it yourself.

WHY IT IS FAST AND LIGHT
  Rust engine      JSON / JSONL / CSV parsing, normalisation, duplicate
                   detection (shingles), validation, split planning, leakage
                   checks, token estimation, format conversion, message
                   editing and template expansion all run in a zero-dependency
                   Rust core called through JNI.
  Native UI        Kotlin + Jetpack Compose only. No WebView, no Google Play
                   Services, no heavy frameworks. Small APK, small RAM use.
  SQLite storage   WAL mode and full-text search (FTS) handle large datasets;
                   history and versions are content-addressed so they cost
                   almost nothing.
  Streaming        Every import format is read as a stream by the Rust engine
                   (by file descriptor, no copy), so memory stays flat whatever
                   the file size: a 5 GB file uses the same RAM as a 5 MB one.

WHAT YOU CAN DO
  Create            Projects and datasets of any kind: conversations (any
                    number of turns), instruction/response, classification,
                    custom fields.
  Quick Write       Type like in a note app: write "User: ..." and press
                    Enter, the next "Assistant:" label appears automatically.
  Edit              Structured record editor with role chips, add / duplicate /
                    split / merge / reorder messages, undo/redo, draft
                    recovery, and a JSON view for full control.
  Import            JSONL, JSON, CSV, TSV, TXT, Markdown, HTML pages, Parquet,
                    and ZIP / TAR / gzip / Zstandard archives of those. Size
                    and decompression limits, corrupt-data detection and
                    per-error logging: a bad line, row group or archive entry
                    is skipped and logged, the rest is still imported.
                    ShareGPT, Alpaca and prompt/completion formats are
                    converted automatically. Markdown conversations are
                    understood.
  Sources           Register a file URL, web page or JSON API. License Gate
                    first (allow / needs your decision / blocked). Resumable,
                    parallel, chunked downloads with retry, Retry-After and
                    rate limiting; SHA-256 verification; source metadata;
                    version history; incremental updates (conditional
                    requests plus content fingerprints). robots.txt is
                    honoured for web pages. API keys are never stored.
  Extract           HTML to clean text: boilerplate, navigation, ads, cookie
                    banners removed; headings kept; Unicode and whitespace
                    normalised; paragraphs rebuilt; sentence boundaries.
  Intelligence      Length, readability, quality, language, source, license,
                    domain, time and duplicate statistics for a dataset.
  Clean             Exact and near-duplicate detection, token-level search,
                    batch operations, template expansion with variables.
  Validate          Required fields, schema kinds, quality reports per record.
  Split             Train / validation / test planning with a leakage check.
  Version           Snapshots, diff between versions, one-tap restore.
  Query             Built-in read-only SQL console over your data.
  Export            JSONL, JSON, CSV, TXT, Markdown with profiles (messages,
                    ShareGPT, Alpaca, ChatML), verified by checksum. Export a
                    whole project as a single ZIP with a manifest.
  Back up           One-file full backup and verified restore.
  Organise          Long-press any dataset for rename, duplicate, lock or
                    delete.

DESIGN
  AMOLED black, iOS-style layout, real liquid-glass surfaces with refraction,
  duotone icon set, smooth motion that respects "Reduce motion". Arabic and
  English interface with correct right-to-left handling.

PRIVACY AND SECURITY
  Everything is stored locally. Network access is used only by the optional
  in-app updater (checks GitHub Releases), by the log-report button you
  press yourself, and by sources you register and fetch yourself. Nothing
  runs in the background. Builds are signed with a persistent key so updates install
  over the previous version. See SECURITY.md.

GET THE APP
  Download the latest signed APK from the Releases page of this repository
  and install it. Later updates come from Settings > App update.

BUILD
  Built automatically by GitHub Actions (Rust tests, cargo-ndk for arm64,
  Gradle release, signed APK published as a Release). Locally:
    cargo ndk -t arm64-v8a -o app/src/main/jniLibs build --release -p dataforge-jni
    gradle :app:assembleRelease
  Engine tests:  cargo test -p dataforge-core

PROJECT LAYOUT
  core/   Rust engine        jni/   JNI bridge
  app/    Android app (Kotlin: data/ ops/ ui/ core/)
  docs/   status and notes   journal/   build log

LICENSE
  Source-available, view only. You may read the code and use the official
  builds; modifying or redistributing it requires written permission from the
  owner. See LICENSE.txt.

(c) 2026 Monsif. All rights reserved.
