# FileManager

**English** | [Русский](#-русский)

Portable file manager for **Windows 11 x64**, written in Rust with a native
`egui` interface. Designed to work alongside Windows Explorer — fast, safe on
your files, and free of a console window.

## Status

Early development — **v0.4: file operations**. See [ROADMAP.md](ROADMAP.md)
for the full plan up to the stable 1.0 release (in Russian).

Working today:

- Directory listing: name / size / type / modified columns, dirs-first sorting,
  double-click to enter, Up / Refresh / address bar, breadcrumbs
- Tabs (restored on restart), lazy folder tree, bookmarks, per-tab back/forward history
- Multi-selection: Ctrl/Shift+click, Ctrl+A, arrow keys, Home/End/PageUp/PageDown
- File operations in background queue: copy / move / delete / rename / new folder & file
  with progress, cancel, overwrite and error dialogs; safe cross-volume move (copy→verify→delete)
- Internal clipboard Ctrl+C/X/V, context menu, properties panel
- Background folder loading with cancellation, virtualized list
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

---

# 🇷🇺 Русский

Portable-файловый менеджер для **Windows 11 x64**, написанный на Rust
с интерфейсом на `egui`. Работает рядом с обычным Проводником Windows —
быстрый, безопасный для файлов и без консольного окна.

## Статус

Ранняя разработка — **готова версия 0.4: файловые операции**. Полный план до
стабильного релиза 1.0 — в [ROADMAP.md](ROADMAP.md).

Уже работает:

- Список файлов: колонки имя / размер / тип / изменён, папки первыми,
  вход по двойному клику, кнопки «Наверх» / «Обновить», адресная строка, хлебные крошки
- Вкладки (восстанавливаются после перезапуска), ленивое дерево папок,
  закладки, история назад/вперёд в каждой вкладке
- Мультивыделение: Ctrl/Shift+клик, Ctrl+A, стрелки, Home/End/PageUp/PageDown
- Файловые операции в фоновой очереди: копирование / перемещение / удаление /
  переименование / новая папка и файл — с прогрессом, отменой, диалогами
  перезаписи и ошибок; безопасный move между томами (copy→verify→delete)
- Внутренний буфер Ctrl+C/X/V, контекстное меню, панель свойств
- Фоновое чтение папок с отменой, виртуализированный список
- GUI-окно без консоли, интерфейс RU/EN, светлая/тёмная/системная тема
- Настройки в `%LOCALAPPDATA%\FileManager\settings.json`
- Файловое логирование в `%LOCALAPPDATA%\FileManager\logs\`
- Защита от второго экземпляра (именованный mutex)
- Встроенный манифест: PerMonitorV2 DPI, поддержка длинных путей
- Portable-режим: файл `filemanager.portable` рядом с `.exe` —
  и все данные хранятся рядом с программой

Планируется к 1.0:

- Вкладки, дерево папок, закладки, история «назад/вперёд»
- Копирование / перемещение / удаление через фоновую очередь
  с прогрессом и обработкой ошибок (`copy → verify → delete`
  при перемещении между томами)
- Стандартная Корзина Windows через `IFileOperation` **и** отдельная
  защищённая корзина приложения с метаданными и безопасным восстановлением
- Буфер обмена `CF_HDROP` совместимый с Проводником, OLE drag-and-drop
- Интеграция в контекстное меню через `HKCU` (без прав администратора)
- Поиск, fuzzy-фильтр, предпросмотр текста и изображений, просмотр ZIP
- Автообновление через отдельный updater с проверкой SHA-256 и подписи

## Технологии

| Слой | Выбор |
|---|---|
| Язык | Rust (edition 2024) |
| UI | `egui` / `eframe` |
| Windows API | crate `windows` |
| Настройки | JSON через `serde` |
| Логирование | `tracing` + `tracing-appender` (ежедневная ротация) |

Структура кода: `src/core` (логика ФС, настройки, i18n — без UI/Win32),
`src/platform/windows` (специфика Win32), `src/ui` (виджеты egui).

## Сборка и запуск

Требования: Rust toolchain (MSVC), Windows 11 x64.

```powershell
cargo build --release
.\target\release\filemanager.exe
```

## Лицензия

Пока не выбрана — будет определена к публичному релизу 1.0.
