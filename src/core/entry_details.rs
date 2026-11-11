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

/// Тяжёлые сведения об одном объекте. Список файлов их не хранит.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryDetails {
    pub size_on_disk: Detail<u64>,
    /// Для папки размер на диске относится только к записи каталога.
    pub folder_contents_not_counted: bool,
    pub accessed: Detail<SystemTime>,
    pub owner: Detail<String>,
    pub access: Detail<AccessList>,
}

impl EntryDetails {
    pub fn unavailable() -> Self {
        Self {
            size_on_disk: Detail::Unavailable,
            folder_contents_not_counted: false,
            accessed: Detail::Unavailable,
            owner: Detail::Unavailable,
            access: Detail::Unavailable,
        }
    }

    /// Ссылку не раскрываем: числа цели нельзя показать как свойства самой ссылки.
    pub fn for_symlink() -> Self {
        Self::unavailable()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symlink_details_do_not_invent_target_numbers() {
        let details = EntryDetails::for_symlink();
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
}
