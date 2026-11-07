use std::io;
use std::process::Command;

const ROOT: &str = r"HKCU\Software\Classes\Directory\shell\FileManager";
const ROOT_BG: &str = r"HKCU\Software\Classes\Directory\Background\shell\FileManager";
const ROOT_DRIVE: &str = r"HKCU\Software\Classes\Drive\shell\FileManager";
const ROOTS: [&str; 3] = [ROOT, ROOT_BG, ROOT_DRIVE];

/// Registers "Открыть в FileManager" in Explorer's folder context menu (HKCU).
pub fn install_context_menu(exe: &std::path::Path, title: &str) -> io::Result<()> {
    install_at(&ROOTS.map(str::to_owned), exe, title)
}

fn install_at(roots: &[String], exe: &std::path::Path, title: &str) -> io::Result<()> {
    let cmd = format!("\"{}\" \"%1\"", exe.display());
    for root in roots {
        run(&["add", root, "/ve", "/d", title, "/f"])?;
        run(&["add", root, "/v", "Icon", "/d", &format!("\"{}\"", exe.display()), "/f"])?;
        run(&["add", &format!("{}\\command", root), "/ve", "/d", &cmd, "/f"])?;
    }
    Ok(())
}

/// Removes the context-menu entries.
pub fn uninstall_context_menu() -> io::Result<()> {
    uninstall_at(&ROOTS.map(str::to_owned))
}

fn uninstall_at(roots: &[String]) -> io::Result<()> {
    for root in roots {
        run(&["delete", root, "/f"])?;
    }
    Ok(())
}

fn run(args: &[&str]) -> io::Result<()> {
    let out = reg_command()?.args(args).output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::Other, String::from_utf8_lossy(&out.stderr).into_owned()))
    }
}

fn reg_command() -> io::Result<Command> {
    Ok(Command::new(
        crate::system_tools::system_executable("reg.exe")?,
    ))
}

#[cfg(test)]
mod tests_v05 {
    use super::*;

    #[test]
    fn installs_and_removes_test_only_registry_keys() {
        let exe = std::env::current_exe().unwrap();
        let base = format!(r"HKCU\Software\Classes\FileManager-Test-{}", std::process::id());
        let roots = [
            format!(r"{base}\Directory\shell\FileManager"),
            format!(r"{base}\Directory\Background\shell\FileManager"),
            format!(r"{base}\Drive\shell\FileManager"),
        ];
        install_at(&roots, &exe, "Open in FileManager").unwrap();
        for k in &roots {
            let q = reg_command().unwrap().args(["query", k]).output().unwrap();
            assert!(q.status.success(), "missing {k}");
            let qc = reg_command()
                .unwrap()
                .args(["query", &format!("{k}\\command"), "/ve"])
                .output()
                .unwrap();
            let out = String::from_utf8_lossy(&qc.stdout);
            assert!(out.contains("%1"), "command lacks %1 in {k}");
        }
        uninstall_at(&roots).unwrap();
        for k in &roots {
            let q = reg_command().unwrap().args(["query", k]).output().unwrap();
            assert!(!q.status.success(), "leftover {k}");
        }
        assert_eq!(ROOTS, [ROOT, ROOT_BG, ROOT_DRIVE]);
    }
}
