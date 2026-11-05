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
mod tests_v05 {
    use super::*;
    use std::process::Command;

    #[test]
    fn installs_all_three_keys() {
        let exe = std::env::current_exe().unwrap();
        install_context_menu(&exe, "Open in FileManager").unwrap();
        for k in [ROOT, ROOT_BG, ROOT_DRIVE] {
            let q = Command::new("reg").args(["query", k]).output().unwrap();
            assert!(q.status.success(), "missing {k}");
            let qc = Command::new("reg").args(["query", &format!("{k}\\command"), "/ve"]).output().unwrap();
            let out = String::from_utf8_lossy(&qc.stdout);
            assert!(out.contains("%1"), "command lacks %1 in {k}");
        }
        uninstall_context_menu().unwrap();
        for k in [ROOT, ROOT_BG, ROOT_DRIVE] {
            let q = Command::new("reg").args(["query", k]).output().unwrap();
            assert!(!q.status.success(), "leftover {k}");
        }
    }
}
