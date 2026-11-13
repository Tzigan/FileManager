use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Значение, которое удалось прочитать, либо честная недоступность.
/// Неподставленное число сюда не попадает.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detail<T> {
    Known(T),
    Unavailable,
}

/// Факт о списке доступа без пересказа конкретных прав.
/// NULL и пустой список в Windows означают разное, поэтому они не склеены.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessList {
    /// Дескриптор без DACL: Windows разрешает доступ всем.
    Null,
    /// DACL есть, записей в нём нет.
    Empty,
    /// Записи есть. Их число — не расшифровка прав.
    Present { entries: u16 },
}

/// Дополнительные атрибуты Windows, полученные без раскрытия ссылки.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExtraFileAttributes {
    pub archive: bool,
    pub compressed: bool,
    pub encrypted: bool,
    pub reparse_point: bool,
}

/// Тяжёлые сведения об одном объекте. Список файлов их не хранит.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryDetails {
    pub size_on_disk: Detail<u64>,
    /// Для папки размер на диске относится только к записи каталога.
    pub folder_contents_not_counted: bool,
    pub accessed: Detail<SystemTime>,
    pub owner: Detail<String>,
    pub access: Detail<AccessList>,
    pub extra_attributes: Detail<ExtraFileAttributes>,
}

impl EntryDetails {
    pub fn unavailable() -> Self {
        Self {
            size_on_disk: Detail::Unavailable,
            folder_contents_not_counted: false,
            accessed: Detail::Unavailable,
            owner: Detail::Unavailable,
            access: Detail::Unavailable,
            extra_attributes: Detail::Unavailable,
        }
    }

    /// Ссылку не раскрываем: числа цели нельзя показать как свойства самой ссылки.
    pub fn for_symlink(extra_attributes: ExtraFileAttributes) -> Self {
        Self {
            extra_attributes: Detail::Known(extra_attributes),
            ..Self::unavailable()
        }
    }
}

/// Текст поля. Недоступность остаётся переданной подписью и не превращается в текст ошибки.
pub fn detail_text<T>(
    detail: &Detail<T>,
    unavailable: &str,
    known: impl FnOnce(&T) -> String,
) -> String {
    match detail {
        Detail::Known(value) => known(value),
        Detail::Unavailable => unavailable.to_owned(),
    }
}

/// Какой путь вообще можно спрашивать. Архив, корзина и элемент архива остаются на лёгких полях.
pub fn heavy_details_path<'a>(
    props_window: Option<&'a Path>,
    in_archive: bool,
    secure_trash: bool,
    selected: Option<&'a Path>,
    selected_is_archive_member: bool,
) -> Option<&'a Path> {
    if in_archive || secure_trash {
        return None;
    }
    if let Some(path) = props_window {
        return Some(path);
    }
    if selected_is_archive_member {
        return None;
    }
    selected
}

/// Что делать с наблюдателем на этом кадре.
#[derive(Debug, PartialEq, Eq)]
pub enum DetailsStep {
    /// Тот же объект уже запрошен или уже показан.
    Keep,
    /// Смотреть больше не на что. Поколение сдвинуто, старый ответ не подойдёт.
    Clear,
    /// Нужен новый запрос именно с этим поколением.
    Start { generation: u64, path: PathBuf },
}

/// Состояние одного запроса. Ответ применяется только к тому же поколению и пути.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailsWatch {
    pub generation: u64,
    pub path: Option<PathBuf>,
    pub pending: bool,
    pub details: Option<EntryDetails>,
}

impl Default for DetailsWatch {
    fn default() -> Self {
        Self {
            generation: 0,
            path: None,
            pending: false,
            details: None,
        }
    }
}

impl DetailsWatch {
    /// Смена выбора или закрытие окна. Повторный вызов с тем же путём новый запрос не создаёт.
    pub fn follow(&mut self, subject: Option<&Path>) -> DetailsStep {
        let Some(subject) = subject else {
            // Сдвиг поколения важен: поздний ответ не совпадёт, даже если путь очищен.
            if self.path.is_some() || self.pending || self.details.is_some() {
                self.generation = self.generation.wrapping_add(1);
            }
            self.path = None;
            self.pending = false;
            self.details = None;
            return DetailsStep::Clear;
        };
        if self.path.as_deref() == Some(subject) && (self.pending || self.details.is_some()) {
            return DetailsStep::Keep;
        }
        self.generation = self.generation.wrapping_add(1);
        self.path = Some(subject.to_path_buf());
        self.pending = true;
        self.details = None;
        DetailsStep::Start {
            generation: self.generation,
            path: subject.to_path_buf(),
        }
    }

    /// `false` — ответ устарел или относится к другому пути. Состояние не меняется.
    pub fn receive(&mut self, generation: u64, path: &Path, details: EntryDetails) -> bool {
        if self.generation == generation && self.path.as_deref() == Some(path) {
            self.details = Some(details);
            self.pending = false;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symlink_details_do_not_invent_target_numbers() {
        let details = EntryDetails::for_symlink(ExtraFileAttributes {
            reparse_point: true,
            ..ExtraFileAttributes::default()
        });
        assert_eq!(details.size_on_disk, Detail::Unavailable);
        assert_eq!(details.owner, Detail::Unavailable);
        assert_eq!(details.access, Detail::Unavailable);
        assert!(!details.folder_contents_not_counted);
    }

    #[test]
    fn missing_object_details_stay_unavailable() {
        let details = EntryDetails::unavailable();
        assert_eq!(details.accessed, Detail::Unavailable);
        assert!(!details.folder_contents_not_counted);
    }

    #[test]
    fn unavailable_owner_or_acl_shows_unavailable_not_raw_error() {
        let details = EntryDetails::unavailable();
        let owner = detail_text(&details.owner, "недоступно", String::clone);
        let access = detail_text(&details.access, "недоступно", |_| "raw os error 5".to_owned());
        assert_eq!(owner, "недоступно");
        assert_eq!(access, "недоступно");
        assert!(!owner.contains("os error"));
        assert!(!access.contains("os error"));
    }

    #[test]
    fn zip_properties_does_not_start_heavy_details_request() {
        let selected = Path::new(r"C:\docs\book.zip");
        assert_eq!(
            heavy_details_path(None, true, false, Some(selected), false),
            None
        );
        assert_eq!(
            heavy_details_path(Some(selected), true, false, Some(selected), false),
            None
        );
        assert_eq!(
            heavy_details_path(None, false, false, Some(selected), true),
            None
        );
    }

    fn known_owner(name: &str) -> EntryDetails {
        EntryDetails {
            owner: Detail::Known(name.to_owned()),
            ..EntryDetails::unavailable()
        }
    }

    #[test]
    fn stale_details_response_does_not_overwrite_new_selection() {
        let file_a = Path::new(r"C:\docs\a.txt");
        let file_b = Path::new(r"C:\docs\b.txt");
        let mut watch = DetailsWatch::default();
        let DetailsStep::Start { generation: first, .. } = watch.follow(Some(file_a)) else {
            panic!("первый файл должен запустить запрос");
        };
        let DetailsStep::Start {
            generation: second, ..
        } = watch.follow(Some(file_b))
        else {
            panic!("второй файл должен запустить новый запрос");
        };
        assert_ne!(first, second);
        assert!(!watch.receive(first, file_a, known_owner("owner-a")));
        assert!(watch.pending);
        assert!(watch.details.is_none());
        assert!(watch.receive(second, file_b, known_owner("owner-b")));
        assert_eq!(
            watch.details.as_ref().and_then(|details| match &details.owner {
                Detail::Known(name) => Some(name.as_str()),
                Detail::Unavailable => None,
            }),
            Some("owner-b")
        );
    }

    #[test]
    fn stale_details_response_does_not_leak_into_props_panel_or_window() {
        let old_path = Path::new(r"C:\docs\old.txt");
        let new_path = Path::new(r"C:\docs\new.txt");
        let mut watch = DetailsWatch::default();
        let DetailsStep::Start {
            generation: old_generation,
            ..
        } = watch.follow(Some(old_path))
        else {
            panic!("старый выбор должен запустить запрос");
        };
        let DetailsStep::Start {
            generation: new_generation,
            ..
        } = watch.follow(Some(new_path))
        else {
            panic!("новый выбор должен сменить поколение");
        };
        assert!(!watch.receive(
            old_generation,
            old_path,
            known_owner("устаревший владелец")
        ));
        assert_eq!(watch.path.as_deref(), Some(new_path));
        assert!(watch.pending);
        assert!(watch.receive(
            new_generation,
            new_path,
            known_owner("актуальный владелец")
        ));
        assert_eq!(
            watch.details.as_ref().and_then(|details| match &details.owner {
                Detail::Known(name) => Some(name.as_str()),
                Detail::Unavailable => None,
            }),
            Some("актуальный владелец")
        );
    }

    #[test]
    fn closing_properties_window_cancels_or_ignores_pending_details() {
        let file = Path::new(r"C:\docs\a.txt");
        let mut watch = DetailsWatch::default();
        let DetailsStep::Start { generation, .. } = watch.follow(Some(file)) else {
            panic!("окно должно запустить запрос");
        };
        assert_eq!(watch.follow(None), DetailsStep::Clear);
        assert!(!watch.pending);
        assert!(watch.path.is_none());
        assert!(!watch.receive(generation, file, known_owner("owner-a")));
        assert!(watch.details.is_none());
        assert_eq!(watch.follow(None), DetailsStep::Clear);
    }

    #[test]
    fn deleted_or_moved_object_does_not_panic_details_request() {
        let details = EntryDetails::unavailable();
        let mut watch = DetailsWatch::default();
        let path = Path::new(r"C:\docs\gone.txt");
        let DetailsStep::Start { generation, .. } = watch.follow(Some(path)) else {
            panic!("запрос должен начаться");
        };
        assert!(watch.receive(generation, path, details));
        assert_eq!(
            watch.details.as_ref().map(|details| &details.owner),
            Some(&Detail::Unavailable)
        );
    }
}
