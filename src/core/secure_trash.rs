//! Защищённая корзина приложения: контролируемое удаление и восстановление.
//!
//! Это не системная Корзина Windows и не криптографическое стирание.
//! Оригинал удаляется только после того, как копия в корзине уже на месте.
//! В пределах тома это переименование. Между томами — копирование, проверка
//! размера и только потом удаление источника.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Имя служебной папки. Меняется в одном месте, а не в интерфейсе.
pub const TRASH_DIR_NAME: &str = ".FileManagerSecureTrash";

/// Раньше корзина по умолчанию вставала на том удалённого файла. Файлы часто
/// лежат на C, поэтому папка появлялась как `C:\.FileManagerSecureTrash`, даже
/// когда программа запущена с другого диска. Теперь default — диск программы.
/// Папка создаётся только в момент удаления, не при старте. Объекты, которые
/// уже лежат на других томах, не переносятся.

/// По умолчанию содержимое хешируется только до этого размера.
/// Большой файл не должен подвешивать удаление. Это не безопасное стирание.
pub const DEFAULT_HASH_MAX_BYTES: u64 = 16 * 1024 * 1024;

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrashStatus {
    InTrash,
    Damaged,
    VolumeUnavailable,
}

/// Как проверялось содержимое. Большой файл не хешируется в окне,
/// поэтому «суммы нет» — это честный статус, а не скрытая гарантия.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IntegrityNote {
    /// Хеш не считался: файл больше порога или порог выключен, перенос был переименованием.
    #[default]
    NotChecked,
    /// Контрольная сумма содержимого записана.
    Hashed,
    /// Между томами файл скопирован и сверен по размеру, без хеша.
    SizeChecked,
}

/// Одно правило для статуса: хеш важнее, иначе копия между томами отмечена сверкой размера.
pub fn integrity_note_for(copied_across_volumes: bool, hashed: bool) -> IntegrityNote {
    if hashed {
        IntegrityNote::Hashed
    } else if copied_across_volumes {
        IntegrityNote::SizeChecked
    } else {
        IntegrityNote::NotChecked
    }
}

/// Ключ подписи, если целостность содержимого не подтверждена хешем.
/// None — хеш есть, отдельная оговорка не нужна.
/// Строка списка корзины. Имя — исходное, не внутреннее имя файла хранения.
/// Список нельзя строить через read_dir: в папке хранения лежат изолированные id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashListRow {
    pub id: String,
    pub name: String,
    /// Расширение исходного имени. Подпись строки его не содержит.
    pub extension: Option<String>,
    pub origin: String,
    pub is_dir: bool,
    pub size: u64,
    pub deleted_at: u64,
    pub stored_path: String,
}

pub fn list_row(record: &TrashRecord) -> TrashListRow {
    let name = if record.original_name.is_empty() {
        record.id.clone()
    } else {
        record.original_name.clone()
    };
    TrashListRow {
        id: record.id.clone(),
        // Тот же расчёт, что у обычной папки: исходное имя карточки, не подпись строки.
        extension: crate::core::fs::listing_extension(record.is_dir, Path::new(&name)),
        name,
        origin: record.original_path.clone(),
        is_dir: record.is_dir,
        size: record.size,
        deleted_at: record.deleted_at,
        stored_path: record.stored_path.clone(),
    }
}

pub fn integrity_status_key(record: &TrashRecord) -> Option<&'static str> {
    if record.integrity.is_some() || record.integrity_note == IntegrityNote::Hashed {
        return None;
    }
    Some(match record.integrity_note {
        IntegrityNote::SizeChecked => "trash.integrity.size",
        IntegrityNote::Hashed | IntegrityNote::NotChecked => "trash.integrity.not_checked",
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrashRecord {
    pub id: String,
    pub original_path: String,
    pub original_name: String,
    pub is_dir: bool,
    pub deleted_at: u64,
    pub size: u64,
    pub stored_path: String,
    /// None — том не удалось опознать, восстановление тогда строже.
    pub volume_id: Option<String>,
    pub status: TrashStatus,
    pub integrity: Option<String>,
    /// Старые карточки без поля читаются как «не проверялась».
    #[serde(default)]
    pub integrity_note: IntegrityNote,
    /// 0 — атрибуты в карточке не записаны, при восстановлении ничего не навязываем.
    #[serde(default)]
    pub attributes: u32,
}

#[derive(Debug)]
pub enum TrashError {
    DamagedMetadata,
    VolumeUnavailable,
    PathEscape,
    SpecialFile,
    Conflict,
    Io(io::Error),
}

impl TrashError {
    pub fn message_key(&self) -> &'static str {
        match self {
            Self::DamagedMetadata => "trash.err.damaged",
            Self::VolumeUnavailable => "trash.err.volume",
            Self::PathEscape => "trash.err.path",
            Self::SpecialFile => "trash.err.special",
            Self::Conflict => "trash.err.conflict",
            Self::Io(_) => "trash.err.io",
        }
    }

    pub fn io_error(&self) -> Option<&io::Error> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl std::fmt::Display for TrashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DamagedMetadata => write!(f, "метаданные повреждены, восстановление остановлено"),
            Self::VolumeUnavailable => write!(f, "исходный том недоступен или заменён"),
            Self::PathEscape => write!(f, "путь восстановления выходит за разрешённый корень"),
            Self::SpecialFile => {
                write!(f, "специальный файл нельзя поместить в корзину как обычный")
            }
            Self::Conflict => write!(f, "по исходному пути уже есть объект"),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

#[derive(Clone, Copy)]
pub enum RestoreConflict {
    NewName,
    Overwrite,
    Skip,
    Cancel,
}

pub struct TrashOptions {
    pub hash_max_bytes: u64,
    pub volume_id: Option<String>,
    /// Запасной корень, если на своём томе нет места. Не используется молча.
    pub fallback_root: Option<PathBuf>,
    pub allow_fallback: bool,
    /// Явный корень корзины. Если задан, он важнее диска программы.
    /// None — корень на диске исполняемого файла, не на томе удаляемого объекта.
    pub storage_root: Option<PathBuf>,
}

pub fn trash_root_for(source: &Path) -> Option<PathBuf> {
    volume_root(source).map(|root| root.join(TRASH_DIR_NAME))
}

/// Диск программы: сначала каталог `current_exe`, если его нет — `current_dir`.
/// Системный диск C здесь не подставляется. Папку эта функция не создаёт.
pub fn program_trash_root() -> Result<PathBuf, TrashError> {
    let anchor = std::env::current_exe()
        .ok()
        .filter(|path| volume_root(path).is_some())
        .or_else(|| std::env::current_dir().ok());
    let Some(anchor) = anchor else {
        return Err(TrashError::Io(io::Error::other(
            "не удалось определить диск программы",
        )));
    };
    trash_root_for(&anchor)
        .ok_or_else(|| TrashError::Io(io::Error::other("не удалось определить диск программы")))
}

/// Помещает объект в корзину. При сбое копирования источник остаётся на месте.
pub fn move_to_trash(source: &Path, options: &TrashOptions) -> Result<TrashRecord, TrashError> {
    let source = absolute_without_dots(source)?;
    let meta = fs::symlink_metadata(&source).map_err(TrashError::Io)?;
    if is_reparse(&meta) || !(meta.is_file() || meta.is_dir()) {
        return Err(TrashError::SpecialFile);
    }
    // Явный путь из настроек/теста важнее. Иначе диск программы, не диск файла и не C.
    let primary = if let Some(root) = options.storage_root.clone() {
        root
    } else {
        program_trash_root()?
    };
    let root = match ensure_trash_root(&primary) {
        Ok(root) => root,
        Err(error) if options.allow_fallback => options
            .fallback_root
            .as_ref()
            .map(|fallback| fallback.join(TRASH_DIR_NAME))
            .ok_or(error)
            .and_then(|root| ensure_trash_root(&root))?,
        Err(error) => return Err(error),
    };
    let id = unique_id();
    let payload = root.join("items").join(&id);
    fs::create_dir_all(payload.parent().unwrap()).map_err(TrashError::Io)?;
    let copied = relocate(&source, &payload)?;
    let size = tree_size(&payload).unwrap_or(0);
    let integrity = integrity_of(&payload, options.hash_max_bytes);
    let record = TrashRecord {
        id: id.clone(),
        original_path: source.to_string_lossy().to_string(),
        original_name: source
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default(),
        is_dir: meta.is_dir(),
        deleted_at: now_secs(),
        size,
        attributes: attributes_of(&meta),
        stored_path: payload.to_string_lossy().to_string(),
        volume_id: options.volume_id.clone(),
        status: TrashStatus::InTrash,
        integrity: integrity.clone(),
        integrity_note: integrity_note_for(copied, integrity.is_some()),
    };
    if write_meta(&root, &record).is_err() {
        // Метаданные не записались: возвращаем объект назад и не оставляем сироту.
        let _ = relocate(&payload, &source);
        return Err(TrashError::Io(io::Error::other("metadata")));
    }
    Ok(record)
}

pub fn list_trash(volumes: &[PathBuf]) -> Vec<TrashRecord> {
    let mut items = Vec::new();
    for volume in volumes {
        let root = volume.join(TRASH_DIR_NAME);
        let meta_dir = root.join("meta");
        if let Ok(entries) = fs::read_dir(&meta_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                match fs::read_to_string(&path).and_then(|raw| {
                    serde_json::from_str::<TrashRecord>(&raw)
                        .map_err(|error| io::Error::other(error))
                }) {
                    Ok(mut record) => {
                        if !Path::new(&record.stored_path).exists() {
                            record.status = TrashStatus::Damaged;
                        }
                        items.push(record);
                    }
                    Err(_) => items.push(TrashRecord {
                        id: path
                            .file_stem()
                            .map(|name| name.to_string_lossy().to_string())
                            .unwrap_or_else(|| "damaged".to_string()),
                        original_path: String::new(),
                        original_name: path
                            .file_stem()
                            .map(|name| name.to_string_lossy().to_string())
                            .unwrap_or_else(|| "повреждённая запись".to_string()),
                        is_dir: false,
                        deleted_at: 0,
                        size: 0,
                        attributes: 0,
                        stored_path: String::new(),
                        volume_id: None,
                        status: TrashStatus::Damaged,
                        integrity: None,
                        integrity_note: IntegrityNote::NotChecked,
                    }),
                }
            }
        }
        // Объект без карточки метаданных тоже виден: восстановить его вслепую нельзя.
        if let Ok(entries) = fs::read_dir(root.join("items")) {
            for entry in entries.flatten() {
                let id = entry.file_name().to_string_lossy().to_string();
                if items.iter().any(|item| item.id == id) {
                    continue;
                }
                items.push(TrashRecord {
                    id,
                    original_path: String::new(),
                    original_name: "повреждённая запись".to_string(),
                    is_dir: entry.path().is_dir(),
                    deleted_at: 0,
                    size: 0,
                    attributes: 0,
                    stored_path: entry.path().to_string_lossy().to_string(),
                    volume_id: None,
                    status: TrashStatus::Damaged,
                    integrity: None,
                    integrity_note: IntegrityNote::NotChecked,
                });
            }
        }
    }
    items.sort_by(|left, right| right.deleted_at.cmp(&left.deleted_at));
    items
}

pub fn restore(
    record: &TrashRecord,
    current_volume_id: Option<&str>,
    allowed_roots: &[PathBuf],
    conflict: Option<RestoreConflict>,
) -> Result<PathBuf, TrashError> {
    if record.status == TrashStatus::Damaged || record.original_path.is_empty() {
        return Err(TrashError::DamagedMetadata);
    }
    match (&record.volume_id, current_volume_id) {
        (Some(stored), Some(current)) if stored != current => {
            return Err(TrashError::VolumeUnavailable);
        }
        (Some(_), None) => return Err(TrashError::VolumeUnavailable),
        (None, _) => {
            // Том не был опознан при удалении: путь всё равно проверяется,
            // но восстановление не идёт вслепую на чужой букве диска.
            if current_volume_id.is_none()
                && volume_root(Path::new(&record.original_path)).is_none()
            {
                return Err(TrashError::VolumeUnavailable);
            }
        }
        _ => {}
    }
    let stored = PathBuf::from(&record.stored_path);
    if !stored.exists() {
        return Err(TrashError::DamagedMetadata);
    }
    if let (Some(expected), Some(actual)) = (&record.integrity, integrity_of(&stored, u64::MAX))
        && expected != &actual
    {
        return Err(TrashError::DamagedMetadata);
    }
    let target = validate_restore_path(Path::new(&record.original_path), allowed_roots)?;
    if target.exists() {
        match conflict {
            Some(RestoreConflict::Skip) | Some(RestoreConflict::Cancel) => {
                return Err(TrashError::Conflict);
            }
            Some(RestoreConflict::NewName) => {
                return finish_restore(record, &stored, &unique_sibling(&target));
            }
            Some(RestoreConflict::Overwrite) => {
                remove_tree(&target)?;
            }
            None => return Err(TrashError::Conflict),
        }
    }
    finish_restore(record, &stored, &target)
}

pub fn purge(record: &TrashRecord) -> Result<(), TrashError> {
    // Сначала объект. Если он не удалился, карточка остаётся и запись не пропадает молча.
    if !record.stored_path.is_empty() {
        remove_tree(&PathBuf::from(&record.stored_path))?;
    }
    if let Some(root) = meta_root_of(record) {
        let meta = root.join("meta").join(format!("{}.json", record.id));
        match fs::remove_file(&meta) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(TrashError::Io(error)),
        }
    }
    Ok(())
}

/// Что именно увидит пользователь в подтверждении удаления навсегда.
pub fn removal_facts(records: &[TrashRecord]) -> Vec<(String, bool, u64, String)> {
    records
        .iter()
        .map(|record| {
            (
                record.original_name.clone(),
                record.is_dir,
                record.size,
                record.original_path.clone(),
            )
        })
        .collect()
}

pub fn removal_totals(records: &[TrashRecord]) -> (usize, u64) {
    (
        records.len(),
        records
            .iter()
            .fold(0u64, |sum, record| sum.saturating_add(record.size)),
    )
}

pub fn empty_trash(volumes: &[PathBuf]) -> Vec<(PathBuf, String, TrashError)> {
    let mut errors = Vec::new();
    for record in list_trash(volumes) {
        if let Err(error) = purge(&record) {
            errors.push((
                PathBuf::from(record.original_path),
                record.original_name,
                error,
            ));
        }
    }
    errors
}

/// Канонический абсолютный путь внутри одного из разрешённых корней.
pub fn validate_restore_path(
    path: &Path,
    allowed_roots: &[PathBuf],
) -> Result<PathBuf, TrashError> {
    if !path.is_absolute() {
        return Err(TrashError::PathEscape);
    }
    for component in path.components() {
        if matches!(component, Component::ParentDir | Component::CurDir) {
            return Err(TrashError::PathEscape);
        }
    }
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component);
        if prefix.exists() {
            let meta = fs::symlink_metadata(&prefix).map_err(TrashError::Io)?;
            if is_reparse(&meta) {
                return Err(TrashError::PathEscape);
            }
        }
    }
    let parent = path.parent().ok_or(TrashError::PathEscape)?;
    let canonical_parent = if parent.exists() {
        fs::canonicalize(parent).map_err(TrashError::Io)?
    } else {
        parent.to_path_buf()
    };
    let full = canonical_parent.join(path.file_name().ok_or(TrashError::PathEscape)?);
    let allowed = allowed_roots.iter().any(|root| {
        let root = fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        full.starts_with(&root)
    });
    if !allowed {
        return Err(TrashError::PathEscape);
    }
    Ok(full)
}

fn finish_restore(
    record: &TrashRecord,
    stored: &Path,
    target: &Path,
) -> Result<PathBuf, TrashError> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(TrashError::Io)?;
    }
    relocate(stored, target)?;
    // Атрибуты возвращаются после успешного переноса. Ноль в карточке значит
    // «не записаны», старую карточку из-за этого не отвергаем.
    if record.attributes != 0 {
        crate::platform::apply_stored_attributes(target, record.attributes)
            .map_err(TrashError::Io)?;
    }
    if let Some(root) = meta_root_of(record) {
        let _ = fs::remove_file(root.join("meta").join(format!("{}.json", record.id)));
    }
    Ok(target.to_path_buf())
}

/// true — объект скопирован между томами. false — переименован на том же томе.
fn relocate(source: &Path, dest: &Path) -> Result<bool, TrashError> {
    match fs::rename(source, dest) {
        Ok(()) => Ok(false),
        Err(error) if is_cross_device(&error) => {
            copy_then_remove(source, dest)?;
            Ok(true)
        }
        Err(error) => Err(TrashError::Io(error)),
    }
}

fn copy_then_remove(source: &Path, dest: &Path) -> Result<(), TrashError> {
    if let Err(error) = copy_tree(source, dest) {
        let _ = remove_tree(dest);
        return Err(TrashError::Io(error));
    }
    match (tree_size(source), tree_size(dest)) {
        (Ok(left), Ok(right)) if left == right => {}
        _ => {
            let _ = remove_tree(dest);
            return Err(TrashError::Io(io::Error::other("size mismatch")));
        }
    }
    // Источник удаляется только после успешного копирования и сверки размера.
    remove_tree(source)
}

fn copy_tree(source: &Path, dest: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(source)?;
    if is_reparse(&meta) || !(meta.is_file() || meta.is_dir()) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "special"));
    }
    if meta.is_dir() {
        fs::create_dir_all(dest)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &dest.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, dest).map(|_| ())
    }
}

/// Безвозвратное удаление: `remove_file` / `remove_dir_all` — это DeleteFileW и
/// RemoveDirectoryW, не Корзина Windows.
fn remove_tree(path: &Path) -> Result<(), TrashError> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(TrashError::Io(error)),
    };
    if meta.is_dir() && !is_reparse(&meta) {
        fs::remove_dir_all(path).map_err(TrashError::Io)
    } else {
        fs::remove_file(path).map_err(TrashError::Io)
    }
}

fn tree_size(path: &Path) -> Result<u64, TrashError> {
    let meta = fs::symlink_metadata(path).map_err(TrashError::Io)?;
    if is_reparse(&meta) {
        return Err(TrashError::SpecialFile);
    }
    if meta.is_file() {
        return Ok(meta.len());
    }
    if !meta.is_dir() {
        return Err(TrashError::SpecialFile);
    }
    let mut total = 0u64;
    for entry in fs::read_dir(path).map_err(TrashError::Io)? {
        total = total.saturating_add(tree_size(&entry.map_err(TrashError::Io)?.path())?);
    }
    Ok(total)
}

fn integrity_of(path: &Path, max_bytes: u64) -> Option<String> {
    let size = tree_size(path).ok()?;
    if size > max_bytes {
        return None;
    }
    let mut hasher = Fnv::new();
    hash_tree(path, path, &mut hasher).ok()?;
    Some(format!("{:016x}", hasher.0))
}

fn hash_tree(root: &Path, path: &Path, hasher: &mut Fnv) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    let relative = path.strip_prefix(root).unwrap_or(path);
    hasher.write(relative.to_string_lossy().as_bytes());
    hasher.write(&meta.len().to_le_bytes());
    if meta.is_dir() {
        let mut names = fs::read_dir(path)?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        names.sort();
        for child in names {
            hash_tree(root, &child, hasher)?;
        }
    } else if meta.is_file() {
        let mut file = fs::File::open(path)?;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.write(&buffer[..read]);
        }
    }
    Ok(())
}

fn write_meta(root: &Path, record: &TrashRecord) -> io::Result<()> {
    let dir = root.join("meta");
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", record.id));
    let mut file = fs::File::create(&path)?;
    file.write_all(serde_json::to_string_pretty(record)?.as_bytes())
}

fn ensure_trash_root(root: &Path) -> Result<PathBuf, TrashError> {
    fs::create_dir_all(root.join("items")).map_err(TrashError::Io)?;
    fs::create_dir_all(root.join("meta")).map_err(TrashError::Io)?;
    Ok(root.to_path_buf())
}

fn meta_root_of(record: &TrashRecord) -> Option<PathBuf> {
    Path::new(&record.stored_path)
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

fn absolute_without_dots(path: &Path) -> Result<PathBuf, TrashError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(TrashError::PathEscape);
    }
    Ok(path.to_path_buf())
}

pub fn allowed_restore_roots(original: &Path) -> Vec<PathBuf> {
    volume_root(original).into_iter().collect()
}

fn volume_root(path: &Path) -> Option<PathBuf> {
    let raw = path.to_string_lossy();
    let text = raw
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .or_else(|| raw.strip_prefix(r"\\?\").map(|rest| rest.to_string()))
        .unwrap_or_else(|| raw.to_string());
    let bytes = text.as_bytes();
    if bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/') {
        return Some(PathBuf::from(format!(
            "{}:\\",
            text.chars().next()?.to_ascii_uppercase()
        )));
    }
    if let Some(rest) = text.strip_prefix(r"\\") {
        let mut parts = rest.split('\\').filter(|part| !part.is_empty());
        let server = parts.next()?;
        let share = parts.next()?;
        return Some(PathBuf::from(format!(r"\\{server}\{share}")));
    }
    None
}

fn unique_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    format!("{}-{}", now_secs(), COUNTER.fetch_add(1, Ordering::Relaxed))
}

fn unique_sibling(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or(Path::new(""));
    let stem = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("restored");
    let extension = path.extension().and_then(|name| name.to_str());
    for index in 1..1000 {
        let name = match extension {
            Some(extension) => format!("{stem} ({index}).{extension}"),
            None => format!("{stem} ({index})"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!("{stem}-restored"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn attributes_of(meta: &fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes()
}

fn is_reparse(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_type().is_symlink() || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn is_cross_device(error: &io::Error) -> bool {
    // ERROR_NOT_SAME_DEVICE. Других ОС в этом модуле нет.
    error.raw_os_error() == Some(17)
}

struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf29ce484222325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trash_io_error_has_separate_user_message_key_and_diagnostic() {
        let error = TrashError::Io(io::Error::other("raw os error 5"));
        assert_eq!(error.message_key(), "trash.err.io");
        assert!(error.to_string().contains("raw os error 5"));
    }

    #[test]
    fn all_trash_error_variants_have_localized_message_key_in_ru_and_en() {
        let errors = [
            TrashError::DamagedMetadata,
            TrashError::VolumeUnavailable,
            TrashError::PathEscape,
            TrashError::SpecialFile,
            TrashError::Conflict,
            TrashError::Io(io::Error::other("diagnostic only")),
        ];
        for json in [
            include_str!("../../i18n/en.json"),
            include_str!("../../i18n/ru.json"),
        ] {
            let labels: serde_json::Value = serde_json::from_str(json).unwrap();
            for error in &errors {
                let message = labels
                    .get(error.message_key())
                    .and_then(|value| value.as_str())
                    .unwrap_or_else(|| panic!("missing localized key {}", error.message_key()));
                assert!(!message.is_empty());
                assert_ne!(message, error.message_key());
            }
        }
    }

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fm-trash-{}", unique_id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn options(root: &Path) -> TrashOptions {
        TrashOptions {
            hash_max_bytes: DEFAULT_HASH_MAX_BYTES,
            volume_id: Some("VOL-1".to_string()),
            fallback_root: None,
            allow_fallback: false,
            storage_root: Some(root.join(TRASH_DIR_NAME)),
        }
    }

    #[test]
    fn file_and_folder_round_trip() {
        let root = scratch();
        let file = root.join("note.txt");
        fs::write(&file, "hello").unwrap();
        let folder = root.join("docs");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("a.txt"), "a").unwrap();
        let file_record = move_to_trash(&file, &options(&root)).unwrap();
        let folder_record = move_to_trash(&folder, &options(&root)).unwrap();
        assert!(!file.exists());
        assert!(!folder.exists());
        let restored = restore(&file_record, Some("VOL-1"), &[root.clone()], None).unwrap();
        assert_eq!(fs::read_to_string(restored).unwrap(), "hello");
        restore(&folder_record, Some("VOL-1"), &[root.clone()], None).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("docs").join("a.txt")).unwrap(),
            "a"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restore_does_not_overwrite_without_a_choice() {
        let root = scratch();
        let file = root.join("note.txt");
        fs::write(&file, "old").unwrap();
        let record = move_to_trash(&file, &options(&root)).unwrap();
        fs::write(&file, "new").unwrap();
        assert!(matches!(
            restore(&record, Some("VOL-1"), &[root.clone()], None),
            Err(TrashError::Conflict)
        ));
        assert_eq!(fs::read_to_string(&file).unwrap(), "new");
        let renamed = restore(
            &record,
            Some("VOL-1"),
            &[root.clone()],
            Some(RestoreConflict::NewName),
        )
        .unwrap();
        assert_ne!(renamed, file);
        assert_eq!(fs::read_to_string(&file).unwrap(), "new");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn relative_parent_and_outside_root_are_rejected() {
        let root = scratch();
        assert!(validate_restore_path(Path::new("relative.txt"), &[root.clone()]).is_err());
        assert!(
            validate_restore_path(Path::new(r"C:\temp\..\Windows\note.txt"), &[root.clone()])
                .is_err()
        );
        let outside = PathBuf::from(r"C:\Windows\note.txt");
        assert!(validate_restore_path(&outside, &[root.clone()]).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn damaged_metadata_is_listed_and_not_restored() {
        let root = scratch();
        let trash = root.join(TRASH_DIR_NAME);
        fs::create_dir_all(trash.join("meta")).unwrap();
        fs::write(trash.join("meta").join("bad.json"), "{").unwrap();
        fs::create_dir_all(trash.join("items").join("orphan")).unwrap();
        let listed = list_trash(&[root.clone()]);
        assert_eq!(listed.len(), 2);
        assert!(
            listed
                .iter()
                .all(|item| item.status == TrashStatus::Damaged)
        );
        for item in &listed {
            assert!(matches!(
                restore(item, Some("VOL-1"), &[root.clone()], None),
                Err(TrashError::DamagedMetadata)
            ));
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn replaced_volume_blocks_restore() {
        let root = scratch();
        let file = root.join("note.txt");
        fs::write(&file, "hello").unwrap();
        let record = move_to_trash(&file, &options(&root)).unwrap();
        assert!(matches!(
            restore(&record, Some("OTHER"), &[root.clone()], None),
            Err(TrashError::VolumeUnavailable)
        ));
        assert!(Path::new(&record.stored_path).exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn failed_copy_keeps_the_original() {
        let root = scratch();
        let file = root.join("note.txt");
        fs::write(&file, "hello").unwrap();
        let blocker = root.join("blocker");
        fs::write(&blocker, "x").unwrap();
        let dest = blocker.join("note.txt");
        assert!(copy_then_remove(&file, &dest).is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn tampered_payload_is_not_restored() {
        let root = scratch();
        let file = root.join("note.txt");
        fs::write(&file, "hello").unwrap();
        let record = move_to_trash(&file, &options(&root)).unwrap();
        fs::write(&record.stored_path, "tampered").unwrap();
        assert!(matches!(
            restore(&record, Some("VOL-1"), &[root.clone()], None),
            Err(TrashError::DamagedMetadata)
        ));
        assert!(!file.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn symlink_inside_the_restore_path_is_rejected() {
        let root = scratch();
        let outside = scratch();
        let link = root.join("link");
        let created = std::os::windows::fs::symlink_dir(&outside, &link);
        if created.is_ok() {
            let target = link.join("file.txt");
            assert!(matches!(
                validate_restore_path(&target, &[root.clone()]),
                Err(TrashError::PathEscape)
            ));
        }
        // Создание ссылки часто запрещено правами. Стандартная точка соединения
        // уже есть в Windows и должна останавливать восстановление.
        let junction = PathBuf::from(r"C:\Documents and Settings");
        if junction.exists() {
            assert!(matches!(
                validate_restore_path(&junction.join("file.txt"), &[PathBuf::from(r"C:\")]),
                Err(TrashError::PathEscape)
            ));
        } else if created.is_err() {
            panic!(
                "не удалось проверить ссылку: нет прав создать её и нет системной точки соединения"
            );
        }
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[test]
    fn purge_and_empty_remove_records() {
        let root = scratch();
        let file = root.join("note.txt");
        fs::write(&file, "hello").unwrap();
        let record = move_to_trash(&file, &options(&root)).unwrap();
        purge(&record).unwrap();
        assert!(list_trash(&[root.clone()]).is_empty());
        fs::write(&file, "again").unwrap();
        move_to_trash(&file, &options(&root)).unwrap();
        assert!(empty_trash(&[root.clone()]).is_empty());
        assert!(list_trash(&[root.clone()]).is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn purge_keeps_the_record_and_reports_when_one_file_is_locked() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = scratch();
        let locked_file = root.join("locked.txt");
        let free_file = root.join("free.txt");
        fs::write(&locked_file, "keep").unwrap();
        fs::write(&free_file, "gone").unwrap();
        let locked = move_to_trash(&locked_file, &options(&root)).unwrap();
        let free = move_to_trash(&free_file, &options(&root)).unwrap();
        let hold = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&locked.stored_path)
            .unwrap();
        let errors = empty_trash(&[root.clone()]);
        assert_eq!(errors.len(), 1);
        assert!(Path::new(&locked.stored_path).exists());
        assert!(!Path::new(&free.stored_path).exists());
        drop(hold);
        purge(&locked).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn removal_facts_name_the_object_its_kind_size_and_origin() {
        let record = TrashRecord {
            id: "1".into(),
            original_path: r"C:\Work\a.txt".into(),
            original_name: "a.txt".into(),
            is_dir: false,
            deleted_at: 1,
            size: 4,
            stored_path: String::new(),
            volume_id: None,
            status: TrashStatus::InTrash,
            integrity: None,
            integrity_note: IntegrityNote::NotChecked,
            attributes: 0,
        };
        let facts = removal_facts(&[record.clone()]);
        assert_eq!(
            facts,
            vec![("a.txt".into(), false, 4, r"C:\Work\a.txt".into())]
        );
        assert_eq!(removal_totals(&[record]), (1, 4));
    }

    #[test]
    fn large_file_is_not_hashed_and_says_integrity_was_not_checked() {
        let root = scratch();
        let file = root.join("big.bin");
        fs::write(&file, b"0123456789").unwrap();
        let mut options = options(&root);
        options.hash_max_bytes = 4;
        let record = move_to_trash(&file, &options).unwrap();
        assert!(record.integrity.is_none());
        assert_eq!(record.integrity_note, IntegrityNote::NotChecked);
        assert_eq!(
            integrity_status_key(&record),
            Some("trash.integrity.not_checked")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cross_volume_without_a_hash_is_marked_size_checked() {
        assert_eq!(integrity_note_for(true, false), IntegrityNote::SizeChecked);
        let record = TrashRecord {
            id: "1".into(),
            original_path: r"D:\a.txt".into(),
            original_name: "a.txt".into(),
            is_dir: false,
            deleted_at: 1,
            size: 9,
            stored_path: String::new(),
            volume_id: Some("VOL".into()),
            status: TrashStatus::InTrash,
            integrity: None,
            integrity_note: integrity_note_for(true, false),
            attributes: 0,
        };
        assert_eq!(integrity_status_key(&record), Some("trash.integrity.size"));
    }

    #[test]
    fn restore_puts_hidden_and_readonly_attributes_back() {
        use std::os::windows::fs::MetadataExt;
        let root = scratch();
        let file = root.join("secret.txt");
        fs::write(&file, "hidden").unwrap();
        crate::platform::set_attributes(&file, true, true).unwrap();
        let record = move_to_trash(&file, &options(&root)).unwrap();
        assert_ne!(record.attributes & 0x3, 0);
        let restored = restore(&record, Some("VOL-1"), &[root.clone()], None).unwrap();
        let attributes = fs::symlink_metadata(&restored).unwrap().file_attributes();
        assert_ne!(attributes & 0x1, 0, "readonly");
        assert_ne!(attributes & 0x2, 0, "hidden");
        crate::platform::set_attributes(&restored, false, false).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn phantom_card_without_a_payload_is_damaged_and_not_restored() {
        let root = scratch();
        let trash = root.join(TRASH_DIR_NAME);
        fs::create_dir_all(trash.join("meta")).unwrap();
        let record = TrashRecord {
            id: "gone".into(),
            original_path: root.join("note.txt").to_string_lossy().to_string(),
            original_name: "note.txt".into(),
            is_dir: false,
            deleted_at: 5,
            size: 4,
            stored_path: trash
                .join("items")
                .join("gone")
                .to_string_lossy()
                .to_string(),
            volume_id: Some("VOL-1".into()),
            status: TrashStatus::InTrash,
            integrity: None,
            integrity_note: IntegrityNote::NotChecked,
            attributes: 0,
        };
        fs::write(
            trash.join("meta").join("gone.json"),
            serde_json::to_string(&record).unwrap(),
        )
        .unwrap();
        let listed = list_trash(&[root.clone()]);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].status, TrashStatus::Damaged);
        assert!(matches!(
            restore(&listed[0], Some("VOL-1"), &[root.clone()], None),
            Err(TrashError::DamagedMetadata)
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn purge_deletes_permanently_and_does_not_use_the_windows_bin() {
        let root = scratch();
        let name = format!("fm-purge-norecycle-{}.txt", unique_id());
        let file = root.join(&name);
        fs::write(&file, "bye").unwrap();
        let record = move_to_trash(&file, &options(&root)).unwrap();
        purge(&record).unwrap();
        assert!(!Path::new(&record.stored_path).exists());
        assert!(!windows_bin_mentions(&name));
        let source = include_str!("secure_trash.rs");
        assert!(!source.contains(&format!("{}{}", "recycle", "_bin")));
        assert!(!source.contains(&format!("{}{}", "IFile", "Operation")));
        assert!(!source.contains(&format!("{}{}", "SHFile", "Operation")));
        let _ = fs::remove_dir_all(root);
    }

    fn windows_bin_mentions(name: &str) -> bool {
        let mut found = false;
        let utf16: Vec<u8> = name
            .encode_utf16()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        let mut pending = vec![PathBuf::from(r"C:\$Recycle.Bin")];
        while let Some(dir) = pending.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                let file_name = path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("");
                if !file_name.starts_with("$I") {
                    continue;
                }
                let Ok(bytes) = fs::read(&path) else {
                    continue;
                };
                if bytes.windows(utf16.len()).any(|window| window == utf16) {
                    found = true;
                }
            }
        }
        found
    }

    #[test]
    fn default_root_follows_the_program_drive_not_the_deleted_file() {
        let program = trash_root_for(Path::new(r"D:\apps\filemanager.exe")).unwrap();
        let deleted_on_c = trash_root_for(Path::new(r"C:\Users\note.txt")).unwrap();
        assert_eq!(program, PathBuf::from(r"D:\.FileManagerSecureTrash"));
        assert_ne!(program, deleted_on_c);
        let cwd_only = trash_root_for(Path::new(r"E:\work")).unwrap();
        assert_eq!(cwd_only, PathBuf::from(r"E:\.FileManagerSecureTrash"));
        let extended = trash_root_for(Path::new(r"\\?\D:\apps\filemanager.exe")).unwrap();
        assert_eq!(extended, PathBuf::from(r"D:\.FileManagerSecureTrash"));
    }

    #[test]
    fn program_trash_root_matches_the_running_executable_and_does_not_create_it() {
        let exe = std::env::current_exe().unwrap();
        let before = program_trash_root().unwrap();
        assert_eq!(before, trash_root_for(&exe).unwrap());
        let existed = before.exists();
        let after = program_trash_root().unwrap();
        assert_eq!(after.exists(), existed);
        let message = TrashError::Io(io::Error::other("не удалось определить диск программы"));
        assert!(message.to_string().contains("диск программы"));
    }

    #[test]
    fn list_row_shows_the_original_name_not_the_storage_name() {
        let record = TrashRecord {
            id: "99".into(),
            original_path: r"D:\docs\отчёт.txt".into(),
            original_name: "отчёт.txt".into(),
            is_dir: false,
            deleted_at: 10,
            size: 4,
            stored_path: r"D:\.FileManagerSecureTrash\items\99".into(),
            volume_id: None,
            status: TrashStatus::InTrash,
            integrity: None,
            integrity_note: IntegrityNote::NotChecked,
            attributes: 0,
        };
        let row = list_row(&record);
        assert_eq!(row.name, "отчёт.txt");
        assert!(!row.name.contains("99"));
        assert_eq!(row.id, "99");
        assert_eq!(row.origin, r"D:\docs\отчёт.txt");
        assert_eq!(row.extension.as_deref(), Some("txt"));
        let decorated = format!("{} · {}: {} • повреждена", row.name, "Откуда", row.origin);
        assert_ne!(row.name, decorated);
        assert!(!row.name.contains('·'));
    }

    #[test]
    fn trash_extension_matches_a_normal_folder_listing() {
        let cases = [
            ("notes.txt", false, Some("txt")),
            ("NOTES.TXT", false, Some("txt")),
            ("archive.tar.gz", false, Some("gz")),
            ("README", false, None),
            (".profile", false, None),
            ("My.Folder", true, None),
        ];
        for (name, is_dir, expected) in cases {
            let from_folder = crate::core::fs::listing_extension(is_dir, Path::new(name));
            let mut record = sample_record(name);
            record.is_dir = is_dir;
            let from_trash = list_row(&record).extension;
            assert_eq!(from_folder.as_deref(), expected, "{name}");
            assert_eq!(from_trash, from_folder, "{name}");
        }
    }

    #[test]
    fn trash_suffix_words_come_from_both_language_files() {
        let ru = include_str!("../../i18n/ru.json");
        let en = include_str!("../../i18n/en.json");
        for key in [
            "\"trash.col.origin\"",
            "\"trash.col.deleted\"",
            "\"trash.status.ok\"",
            "\"trash.status.damaged\"",
            "\"trash.status.volume\"",
            "\"trash.integrity.not_checked\"",
            "\"trash.integrity.size\"",
        ] {
            assert!(ru.contains(key), "{key}");
            assert!(en.contains(key), "{key}");
        }
    }

    #[test]
    fn trash_folder_size_stays_in_the_card_for_the_empty_confirmation() {
        let mut record = sample_record("Папка");
        record.is_dir = true;
        record.size = 48;
        let row = list_row(&record);
        assert!(row.is_dir);
        assert_eq!(row.size, 48);
        assert_eq!(removal_totals(&[record]).1, 48);
        assert!(row.extension.is_none());
    }

    fn sample_record(name: &str) -> TrashRecord {
        TrashRecord {
            id: "1".into(),
            original_path: format!(r"D:\docs\{name}"),
            original_name: name.to_owned(),
            is_dir: false,
            deleted_at: 10,
            size: 4,
            stored_path: r"D:\.FileManagerSecureTrash\items\1".into(),
            volume_id: None,
            status: TrashStatus::InTrash,
            integrity: None,
            integrity_note: IntegrityNote::NotChecked,
            attributes: 0,
        }
    }

    #[test]
    fn trash_module_has_no_unix_cfg_branch() {
        let source = include_str!("secure_trash.rs");
        let unix_cfg = format!("{}{}", "cfg(", "unix)");
        let other_os = format!("{}{}", "cfg(not(", "windows))");
        let target_os = format!("{}{}", "cfg(", "target_os");
        assert!(!source.contains(&unix_cfg));
        assert!(!source.contains(&other_os));
        assert!(!source.contains(&target_os));
    }
}
