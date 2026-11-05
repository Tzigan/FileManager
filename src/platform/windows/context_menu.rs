use std::io;
use std::process::Command;

const ROOT: &str = r"HKCU\Software\Classes\Directory\shell\FileManager";
const ROOT_BG: &str = r"HKCU\Software\Classes\Directory\Background\shell\FileManager";
const ROOT_DRIVE: &str = r"HKCU\Software\Classes\Drive\shell\FileManager";

/// Registers "Открыть в FileManager" in Explorer's folder context menu (HKCU).
pub fn install_context_menu(exe: &std::path::Path, title: &str) -> io::Result<()> {
    let cmd = format!("\"{}\" \"%1\"", exe.display());
    for root in [ROOT, ROOT_BG, ROOT_DRIVE] {
        run(&["add", root, "/ve", "/d", title, "/f"])?;
        run(&["add", root, "/v", "Icon", "/d", &format!("\"{}\"", exe.display()), "/f"])?;
        run(&["add", &format!("{}\\command", root), "/ve", "/d", &cmd, "/f"])?;
    }
    Ok(())
}

/// Removes the context-menu entries.
pub fn uninstall_context_menu() -> io::Result<()> {
    for root in [ROOT, ROOT_BG, ROOT_DRIVE] {
        run(&["delete", root, "/f"])?;
    }
    Ok(())
}

fn run(args: &[&str]) -> io::Result<()> {
    let out = Command::new("reg").args(args).output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::Other, String::from_utf8_lossy(&out.stderr).into_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn install_query_uninstall() {
        let exe = std::env::current_exe().unwrap();
        install_context_menu(&exe, "Open in FileManager").unwrap();
        let q = Command::new("reg").args(["query", ROOT]).output().unwrap();
        assert!(q.status.success());
        uninstall_context_menu().unwrap();
        let q2 = Command::new("reg").args(["query", ROOT]).output().unwrap();
        assert!(!q2.status.success());
    }
}
