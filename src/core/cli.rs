use std::path::PathBuf;

/// Разбирает путь, `--folder <путь>` и `--select <файл>`.
/// Принимает существующие папки и ZIP-архивы.
pub fn parse_args(args: &[String]) -> (Option<PathBuf>, Option<String>) {
    let mut initial_path = None;
    let mut initial_select = None;
    let mut i = 0;
    while i < args.len() {
        let p = PathBuf::from(&args[i]);
        match args[i].as_str() {
            "--folder" if i + 1 < args.len() => {
                initial_path = Some(PathBuf::from(&args[i + 1]));
                i += 1;
            }
            "--select" if i + 1 < args.len() => {
                let f = PathBuf::from(&args[i + 1]);
                if let (Some(parent), Some(name)) = (f.parent(), f.file_name()) {
                    initial_path = Some(parent.to_path_buf());
                    initial_select = Some(name.to_string_lossy().into_owned());
                }
                i += 1;
            }
            _ if is_openable_path(&p) => initial_path = Some(p),
            _ => {}
        }
        i += 1;
    }
    (initial_path.filter(|path| is_openable_path(path)), initial_select)
}

fn is_openable_path(path: &std::path::Path) -> bool {
    path.is_dir()
        || (path.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("zip")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn temp_dir() -> PathBuf {
        let id = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("File Manager CLI {id}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn folder_flag_sets_path() {
        let (path, sel) = parse_args(&s(&["--folder", "C:\\Windows"]));
        assert_eq!(path.as_deref(), Some(std::path::Path::new("C:\\Windows")));
        assert!(sel.is_none());
    }

    #[test]
    fn select_flag_sets_parent_and_name() {
        let (path, sel) = parse_args(&s(&["--select", "C:\\Windows\\explorer.exe"]));
        assert_eq!(path.as_deref(), Some(std::path::Path::new("C:\\Windows")));
        assert_eq!(sel.as_deref(), Some("explorer.exe"));
    }

    #[test]
    fn positional_dir_sets_path() {
        let (path, _) = parse_args(&s(&["C:\\Windows"]));
        assert_eq!(path.as_deref(), Some(std::path::Path::new("C:\\Windows")));
    }

    #[test]
    fn positional_zip_archive_sets_path() {
        let dir = temp_dir();
        let archive = dir.join("files.ZIP");
        std::fs::write(&archive, b"not decoded by argument parsing").unwrap();
        let (path, _) = parse_args(&[archive.to_string_lossy().into_owned()]);
        assert_eq!(path, Some(archive));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn nonexistent_and_flags_ignored() {
        let (path, sel) = parse_args(&s(&["--bogus", "Z:\\no\\such\\dir"]));
        assert!(path.is_none() && sel.is_none());
    }

    #[test]
    fn flags_preserve_paths_with_spaces_and_select_the_requested_file() {
        let dir = temp_dir();
        let file = dir.join("selected file.txt");
        std::fs::write(&file, b"fixture").unwrap();

        let folder_arg = dir.to_string_lossy().into_owned();
        let (folder, selected) = parse_args(&["--folder".into(), folder_arg.clone()]);
        assert_eq!(folder, Some(dir.clone()));
        assert_eq!(selected, None);

        let file_arg = file.to_string_lossy().into_owned();
        let (parent, selected) = parse_args(&["--select".into(), file_arg]);
        assert_eq!(parent, Some(dir.clone()));
        assert_eq!(selected.as_deref(), Some("selected file.txt"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unknown_or_incomplete_flags_do_not_hide_later_valid_paths() {
        let dir = temp_dir();
        let arg = dir.to_string_lossy().into_owned();
        let (path, selected) = parse_args(&["--unknown".into(), arg.clone(), "--folder".into()]);
        assert_eq!(path, Some(dir.clone()));
        assert_eq!(selected, None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
