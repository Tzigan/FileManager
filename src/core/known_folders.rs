use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemBookmarkKind {
    RecycleBin,
    /// Корзина приложения. Всегда открывается как вкладка со списком карточек.
    SecureTrash,
    Downloads,
    Desktop,
    Documents,
    Pictures,
    Music,
    Videos,
}

impl SystemBookmarkKind {
    pub fn label_key(self) -> &'static str {
        match self {
            Self::RecycleBin => "bookmark.recycle_bin",
            Self::SecureTrash => "bookmark.secure_trash",
            Self::Downloads => "bookmark.downloads",
            Self::Desktop => "bookmark.desktop",
            Self::Documents => "bookmark.documents",
            Self::Pictures => "bookmark.pictures",
            Self::Music => "bookmark.music",
            Self::Videos => "bookmark.videos",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SystemBookmarkTarget {
    Folder(PathBuf),
    /// Известная папка ОС не вернула путь; строка остаётся видимой, но недоступной.
    UnavailableFolder,
    RecycleBin,
    /// Не папка хранения. Вкладка читает карточки, а не readdir.
    SecureTrash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemBookmark {
    pub kind: SystemBookmarkKind,
    pub target: SystemBookmarkTarget,
}

/// Получает системные пути через известные папки ОС, не собирая их из строк.
pub fn system_bookmarks() -> Vec<SystemBookmark> {
    let mut bookmarks = Vec::new();

    // Корзина — виртуальный shell-объект, а не папка в профиле пользователя.
    #[cfg(windows)]
    bookmarks.push(SystemBookmark {
        kind: SystemBookmarkKind::RecycleBin,
        target: SystemBookmarkTarget::RecycleBin,
    });
    bookmarks.push(SystemBookmark {
        kind: SystemBookmarkKind::SecureTrash,
        target: SystemBookmarkTarget::SecureTrash,
    });

    let dirs = directories::UserDirs::new();
    for (kind, path) in [
        (
            SystemBookmarkKind::Downloads,
            dirs.as_ref().and_then(|dirs| dirs.download_dir()),
        ),
        (
            SystemBookmarkKind::Desktop,
            dirs.as_ref().and_then(|dirs| dirs.desktop_dir()),
        ),
        (
            SystemBookmarkKind::Documents,
            dirs.as_ref().and_then(|dirs| dirs.document_dir()),
        ),
        (
            SystemBookmarkKind::Pictures,
            dirs.as_ref().and_then(|dirs| dirs.picture_dir()),
        ),
        (
            SystemBookmarkKind::Music,
            dirs.as_ref().and_then(|dirs| dirs.audio_dir()),
        ),
        (
            SystemBookmarkKind::Videos,
            dirs.as_ref().and_then(|dirs| dirs.video_dir()),
        ),
    ] {
        bookmarks.push(folder_bookmark(kind, path));
    }
    bookmarks
}

/// Защищённая корзина всегда открывается новой вкладкой. Остальные закладки
/// слушаются клика: текущая вкладка или новая.
pub fn bookmark_forces_new_tab(target: &SystemBookmarkTarget) -> bool {
    matches!(target, SystemBookmarkTarget::SecureTrash)
}

fn folder_bookmark(kind: SystemBookmarkKind, path: Option<&Path>) -> SystemBookmark {
    SystemBookmark {
        kind,
        target: path.map_or(SystemBookmarkTarget::UnavailableFolder, |path| {
            SystemBookmarkTarget::Folder(path.to_path_buf())
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_known_folder_keeps_a_non_creating_bookmark_target() {
        let known_path = Path::new(r"C:\Users\Example\Downloads");
        assert_eq!(
            folder_bookmark(SystemBookmarkKind::Downloads, Some(known_path)),
            SystemBookmark {
                kind: SystemBookmarkKind::Downloads,
                target: SystemBookmarkTarget::Folder(known_path.to_path_buf()),
            }
        );
        assert_eq!(
            folder_bookmark(SystemBookmarkKind::Downloads, None).target,
            SystemBookmarkTarget::UnavailableFolder
        );
    }

    #[cfg(windows)]
    #[test]
    fn recycle_bookmark_uses_the_shell_target() {
        assert!(system_bookmarks().iter().any(|bookmark| {
            bookmark.kind == SystemBookmarkKind::RecycleBin
                && bookmark.target == SystemBookmarkTarget::RecycleBin
        }));
    }

    #[cfg(windows)]
    #[test]
    fn secure_trash_bookmark_is_its_own_target() {
        assert!(system_bookmarks().iter().any(|bookmark| {
            bookmark.kind == SystemBookmarkKind::SecureTrash
                && bookmark.target == SystemBookmarkTarget::SecureTrash
        }));
        assert_ne!(
            SystemBookmarkKind::SecureTrash.label_key(),
            SystemBookmarkKind::RecycleBin.label_key()
        );
        assert!(bookmark_forces_new_tab(&SystemBookmarkTarget::SecureTrash));
        assert!(!bookmark_forces_new_tab(&SystemBookmarkTarget::RecycleBin));
    }
}
