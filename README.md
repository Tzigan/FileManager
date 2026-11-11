# FileManager

**English** | [Русский](#-русский)

Portable file manager for **Windows 11 x64** only, written in Rust with a native
`egui` interface. Designed to work alongside Windows Explorer — fast, safe on
your files, and free of a console window.

## Status

Early development — **v0.8.0: protected in-app trash**. See [ROADMAP.md](ROADMAP.md)
for the full plan up to the stable 1.0 release (in Russian).

Working today:

- Directory listing: name / size / type / modified columns, dirs-first sorting,
  grouping, table / list / icons views, double-click to enter, Up / Refresh /
  address bar, breadcrumbs
- Tabs (restored on restart), lazy folder tree with a disk-usage bar, bookmarks,
  per-tab back/forward history
- Multi-selection: Ctrl/Shift+click, Ctrl+A, arrow keys, Home/End/PageUp/PageDown
- File operations in a background queue: copy / move / delete / rename / new folder
  and file, with progress, cancel, overwrite and error dialogs; a cross-volume move
  is copy, then size check, then delete
- Clipboard Ctrl+C/X/V in `CF_HDROP`, so files copy to and from Explorer; files can
  be dropped in from Windows
- Folder context-menu integration via `HKCU` (no administrator rights)
- Properties panel and an Alt+Enter properties window; About window with an update check
- Search by name in the current folder or subfolders, fuzzy match, filters by
  extension, size and date; text and image preview, background thumbnails
- ZIP opens as a folder; files and folders extract to a chosen directory
- Windows Recycle Bin via `IFileOperation`, plus a separate protected app trash:
  metadata, restore that does not overwrite without a choice, permanent delete that
  does not go through the Windows bin
- Delete mode: Windows Recycle Bin, the app trash, ask every time, or always the
  app trash for listed folders
- The app trash lives on the drive of the running program, not on the drive of the
  deleted file and not on a hardcoded `C:`. The system bookmark opens it in a new
  tab from trash cards, with the same extension rules as a normal folder
- Settings window split into General, Files, Folder tree, Navigation, Trash and Updates
- GUI window (no console), RU/EN interface, light/dark/system theme
- Settings in `%LOCALAPPDATA%\FileManager\settings.json`
- File logging in `%LOCALAPPDATA%\FileManager\logs\`
- Single-instance detection (named mutex)
- Embedded manifest: PerMonitorV2 DPI awareness, long-path awareness
- Windows is the only target: do not add `cfg(unix)` / `cfg(not(windows))` branches in new modules
- Portable mode: place a `filemanager.portable` file next to the `.exe`
  and all data stays beside the executable
- Auto-update through a separate `FileManagerUpdater.exe`: SHA-256, Authenticode
  check of both executables, rollback on failure, optional restart after install

Still planned before 1.0:

- Code signing and a SmartScreen check
- Directory watching, a directory cache, and stress tests on very large folders
- Log export and a settings reset
- Dragging files from the app into other Windows windows

## Tech stack

| Layer | Choice |
|---|---|
| Language | Rust (edition 2024) |
| UI | `egui` / `eframe` 0.37 |
| Windows API | `windows` crate |
| Settings | JSON via `serde` |
| Logging | `tracing` + `tracing-appender` (daily rolling files) |

Code layout: `src/core` (FS logic, history, settings, i18n — no UI/Win32),
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

Portable-файловый менеджер только для **Windows 11 x64** (другие ОС не поддерживаются), написанный на Rust
с интерфейсом на `egui`. Работает рядом с обычным Проводником Windows —
быстрый, безопасный для файлов и без консольного окна.

## Статус

Ранняя разработка — **версия 0.8.0: защищённая корзина приложения**. Полный план до
стабильного релиза 1.0 — в [ROADMAP.md](ROADMAP.md).

Уже работает:

- Список файлов: колонки имя / размер / тип / изменён, папки первыми, группировка,
  режимы таблица / список / значки, вход по двойному клику, «Наверх» / «Обновить»,
  адресная строка, хлебные крошки
- Вкладки (восстанавливаются после перезапуска), ленивое дерево папок с полоской
  занятости диска, закладки, история назад/вперёд в каждой вкладке
- Мультивыделение: Ctrl/Shift+клик, Ctrl+A, стрелки, Home/End/PageUp/PageDown
- Файловые операции в фоновой очереди: копирование / перемещение / удаление /
  переименование / новая папка и файл — с прогрессом, отменой, диалогами
  перезаписи и ошибок; перемещение между томами идёт как копирование, проверка размера и удаление
- Буфер Ctrl+C/X/V в формате `CF_HDROP`: файлы вставляются в Проводник и из него;
  файлы можно перетащить из Windows в приложение
- Пункт контекстного меню папок через `HKCU`, без прав администратора
- Панель свойств и окно свойств по Alt+Enter; окно «О программе» с проверкой обновления
- Поиск по имени в текущей папке и в подпапках, fuzzy-поиск, фильтры по расширению,
  размеру и дате; предпросмотр текста и изображений, эскизы в фоне
- ZIP открывается как папка; файлы и папки извлекаются в выбранный каталог
- Стандартная Корзина Windows через `IFileOperation` и отдельная защищённая корзина
  приложения: метаданные, восстановление без затирания без спроса, удаление навсегда
  мимо Корзины Windows
- Режим удаления: Корзина Windows, корзина приложения, спрашивать каждый раз
  или всегда корзина приложения для перечисленных папок
- Корзина по умолчанию лежит на диске запущенной программы, а не на диске удалённого
  файла и не на жёстко заданном `C:`. Системная закладка открывает её новой вкладкой
  по карточкам, расширение считается так же, как в обычной папке
- Окно настроек разделено на «Общие», «Файлы», «Дерево папок», «Навигация», «Корзина» и «Обновления»
- GUI-окно без консоли, интерфейс RU/EN, светлая / тёмная / системная тема
- Настройки в `%LOCALAPPDATA%\FileManager\settings.json`
- Файловое логирование в `%LOCALAPPDATA%\FileManager\logs\`
- Защита от второго экземпляра (именованный mutex)
- Встроенный манифест: PerMonitorV2 DPI, поддержка длинных путей
- Windows — единственная цель: в новых модулях не добавлять ветки `cfg(unix)` / `cfg(not(windows))`
- Portable-режим: файл `filemanager.portable` рядом с `.exe` —
  и все данные хранятся рядом с программой
- Автообновление отдельным `FileManagerUpdater.exe`: SHA-256, проверка Authenticode
  обоих exe, откат при ошибке, по желанию перезапуск после установки

До версии 1.0 ещё в плане:

- Подпись кода и проверка SmartScreen
- Наблюдение за папкой, кэш каталогов и стресс-тесты очень больших папок
- Экспорт логов и сброс настроек
- Перетаскивание файлов из приложения в другие окна Windows

## Технологии

| Слой | Выбор |
|---|---|
| Язык | Rust (edition 2024) |
| UI | `egui` / `eframe` 0.37 |
| Windows API | crate `windows` |
| Настройки | JSON через `serde` |
| Логирование | `tracing` + `tracing-appender` (ежедневная ротация) |

Структура кода: `src/core` (логика ФС, история, настройки, i18n — без UI и Win32),
`src/platform/windows` (специфика Win32), `src/ui` (виджеты egui).

## Сборка и запуск

Требования: Rust toolchain (MSVC), Windows 11 x64.

```powershell
cargo build --release
.\target\release\filemanager.exe
```

## Лицензия

Пока не выбрана — будет определена к публичному релизу 1.0.
