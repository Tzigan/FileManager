# FileManager — сведения для агентов

## Сборка и проверка

```powershell
cargo build            # dev-сборка
cargo build --release  # релизный exe → target\release\filemanager.exe
cargo test             # unit-тесты core
```

Перед релизной сборкой убедиться, что `filemanager.exe` не запущен
(иначе линкер упадёт с `os error 5`).

## Структура

- `src/core` — логика ФС, история, настройки, i18n, форматирование. Без egui и Win32.
- `src/platform/windows` — Win32-специфика (single instance, диски, локальное время).
- `src/ui` — виджеты egui (app, list, tree, props, tabs). UI только вызывает core/platform.

## Соглашения

- **Сообщения коммитов — на русском языке.**
- **Коммит = заголовок + описание того, что сделано** (описание попадает в changelog релиза).
- egui/eframe 0.37: `App::ui(&mut Ui, &mut Frame)`, панели — `egui::Panel::top/bottom/left/right(id)`.
- Строки интерфейса — только через `i18n/*.json` + `t("key")`, без хардкода.
- Выполненные пункты в `ROADMAP.md` помечаются `✅`.
- Токены/секреты не коммитить; локальные хуки живут в `.git/hooks` (не версионируются).
