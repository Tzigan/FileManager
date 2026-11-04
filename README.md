# FileManager

Portable file manager for **Windows 11 x64**, written in Rust with a native
`egui` interface. Designed to work alongside Windows Explorer — fast, safe on
your files, and free of a console window.

> Статус: ранняя разработка. Дорожная карта до версии 1.0 — в [ROADMAP.md](ROADMAP.md) (на русском).

## Status

Early development — **v0.1 skeleton complete**. See [ROADMAP.md](ROADMAP.md)
for the full plan up to the stable 1.0 release.

Working today:

- GUI window (no console subsystem), RU/EN interface, light/dark/system theme
- Settings persisted to `%LOCALAPPDATA%\FileManager\settings.json`
- File logging to `%LOCALAPPDATA%\FileManager\logs\`
- Single-instance detection (named mutex)
- Embedded manifest: PerMonitorV2 DPI awareness, long-path awareness
- Portable mode: place a `filemanager.portable` file next to the `.exe`
  and all data stays beside the executable

Planned for 1.0:

- Tabs, folder tree, bookmarks, back/forward history
- Copy / move / delete with a background operation queue, progress and
  error handling (`copy → verify → delete` for cross-volume moves)
- Windows Recycle Bin via `IFileOperation` **and** a separate protected
  in-app trash with metadata and safe restore
- `CF_HDROP` clipboard interop with Explorer, OLE drag-and-drop
- Context-menu integration via `HKCU` (no admin required)
- Search, fuzzy filter, text/image preview, ZIP browsing
- Auto-update via a separate updater with SHA-256 + signature checks

## Tech stack

| Layer | Choice |
|---|---|
| Language | Rust (edition 2024) |
| UI | `egui` / `eframe` |
| Windows API | `windows` crate |
| Settings | JSON via `serde` |
| Logging | `tracing` + `tracing-appender` (daily rolling files) |

Code layout: `src/core` (FS logic, settings, i18n — no UI/Win32),
`src/platform/windows` (Win32 specifics), `src/ui` (egui widgets).

## Build & run

Requirements: Rust toolchain (MSVC), Windows 11 x64.

```powershell
cargo build --release
.\target\release\filemanager.exe
```

## License

TBD — will be chosen before the public 1.0 release.
