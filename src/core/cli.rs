use std::path::PathBuf;

/// Parses CLI args: positional path, `--folder <dir>`, `--select <file>`.
/// Explorer's context menu passes "%1" positionally.
/// Returns (initial_path, initial_select_name); paths that are not
/// existing directories are filtered out.
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
            _ if p.is_dir() => initial_path = Some(p),
            _ => {}
        }
        i += 1;
    }
    (initial_path.filter(|p: &PathBuf| p.is_dir()), initial_select)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
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
    fn nonexistent_and_flags_ignored() {
        let (path, sel) = parse_args(&s(&["--bogus", "Z:\\no\\such\\dir"]));
        assert!(path.is_none() && sel.is_none());
    }
}
