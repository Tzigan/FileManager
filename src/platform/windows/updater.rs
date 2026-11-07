use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};

use super::update_support::{
    UPDATE_STAGE_ENV, UPDATE_STARTED_MARKER, UPDATE_UPDATER_PID_ENV, validate_stage_files,
    verify_stage_signatures, wait_for_process_exit,
};

const BACKUP_PREFIX: &str = ".filemanager-update-backup-";
const BACKUP_MARKER: &str = ".filemanager-update-backup";
const UPDATER_FILE: &str = "FileManagerUpdater.exe";
const APP_FILE: &str = "FileManager.exe";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
struct UpdaterArgs {
    process_id: u32,
    target_exe: PathBuf,
    stage_dir: PathBuf,
}

#[derive(Debug)]
struct InstallTransaction {
    target_exe: PathBuf,
    updater_exe: PathBuf,
    backup_dir: PathBuf,
    backup_app: PathBuf,
    backup_updater: PathBuf,
    new_app: PathBuf,
    new_updater: PathBuf,
    had_updater: bool,
    new_app_created: bool,
    new_updater_created: bool,
    app_backed_up: bool,
    updater_backed_up: bool,
    app_installed: bool,
    updater_installed: bool,
}

pub fn run() -> anyhow::Result<()> {
    let args = parse_args(&std::env::args_os().skip(1).collect::<Vec<_>>())?;
    validate_target(&args.target_exe)?;

    // Never rename the main executable until the process that loaded it is gone.
    wait_for_process_exit(args.process_id)?;
    if let Err(error) = validate_stage_files(&args.stage_dir) {
        relaunch_previous(&args.target_exe)?;
        return Err(error);
    }
    if let Err(error) = verify_stage_signatures(&args.stage_dir) {
        relaunch_previous(&args.target_exe)?;
        return Err(error);
    }
    let transaction = match install_binaries(&args.target_exe, &args.stage_dir) {
        Ok(transaction) => transaction,
        Err(error) => return Err(error),
    };

    let mut child = match Command::new(&transaction.target_exe)
        .env(UPDATE_STAGE_ENV, &args.stage_dir)
        .env(UPDATE_UPDATER_PID_ENV, std::process::id().to_string())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            rollback(&transaction)?;
            relaunch_previous(&transaction.target_exe)?;
            return Err(error).context("launch updated FileManager");
        }
    };

    if let Err(error) = wait_for_startup(&mut child, &args.stage_dir.join(UPDATE_STARTED_MARKER)) {
        if child.try_wait()?.is_none() {
            let _ = child.kill();
            let _ = child.wait();
        }
        rollback(&transaction)?;
        relaunch_previous(&transaction.target_exe)?;
        return Err(error);
    }

    // The new process has rendered its first frame; old binaries are no longer
    // needed for rollback. The stage directory is removed after this updater exits.
    let _ = fs::remove_dir_all(&transaction.backup_dir);
    Ok(())
}

fn relaunch_previous(target_exe: &Path) -> anyhow::Result<()> {
    Command::new(target_exe)
        .spawn()
        .context("relaunch previous FileManager after rollback")?;
    Ok(())
}

pub fn show_error(message: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{
        MB_ICONERROR, MB_OK, MESSAGEBOX_STYLE, MessageBoxW,
    };
    use windows::core::HSTRING;

    unsafe {
        let _ = MessageBoxW(
            None,
            &HSTRING::from(message),
            &HSTRING::from("FileManager Update"),
            MESSAGEBOX_STYLE(MB_OK.0 | MB_ICONERROR.0),
        );
    }
}

fn parse_args(args: &[OsString]) -> anyhow::Result<UpdaterArgs> {
    let mut process_id = None;
    let mut target_exe = None;
    let mut stage_dir = None;
    let mut index = 0;
    while index < args.len() {
        let name = args[index].to_string_lossy();
        let value = args
            .get(index + 1)
            .context("missing updater argument value")?;
        match name.as_ref() {
            "--pid" => process_id = Some(value.to_string_lossy().parse::<u32>()?),
            "--target" => target_exe = Some(PathBuf::from(value)),
            "--stage" => stage_dir = Some(PathBuf::from(value)),
            _ => bail!("unknown updater argument: {name}"),
        }
        index += 2;
    }
    Ok(UpdaterArgs {
        process_id: process_id.context("missing --pid")?,
        target_exe: target_exe.context("missing --target")?,
        stage_dir: stage_dir.context("missing --stage")?,
    })
}

fn validate_target(target_exe: &Path) -> anyhow::Result<()> {
    if !target_exe.is_file() {
        bail!("installed FileManager executable is missing");
    }
    let file_name = target_exe
        .file_name()
        .and_then(|name| name.to_str())
        .context("invalid target executable name")?;
    if !target_exe
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        || file_name.eq_ignore_ascii_case(UPDATER_FILE)
    {
        bail!("target path is not a FileManager application executable");
    }
    Ok(())
}

fn install_binaries(target_exe: &Path, stage_dir: &Path) -> anyhow::Result<InstallTransaction> {
    let install_dir = target_exe
        .parent()
        .context("target executable has no parent")?;
    validate_target(target_exe)?;
    validate_stage_files(stage_dir)?;

    let id = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let backup_dir = install_dir.join(format!("{BACKUP_PREFIX}{}{id}", std::process::id()));
    if let Err(error) = fs::create_dir(&backup_dir) {
        relaunch_previous(target_exe)?;
        return Err(error)
            .with_context(|| format!("create rollback directory {}", backup_dir.display()));
    }
    if let Err(error) =
        File::create(backup_dir.join(BACKUP_MARKER)).and_then(|file| file.sync_all())
    {
        let _ = fs::remove_file(backup_dir.join(BACKUP_MARKER));
        let _ = fs::remove_dir(&backup_dir);
        relaunch_previous(target_exe)?;
        return Err(error).context("mark update rollback directory");
    }

    let updater_exe = install_dir.join(UPDATER_FILE);
    let backup_app = backup_dir.join(target_exe.file_name().unwrap());
    let backup_updater = backup_dir.join(UPDATER_FILE);
    let new_app = install_dir.join(format!(".filemanager-new-app-{id}.tmp"));
    let new_updater = install_dir.join(format!(".filemanager-new-updater-{id}.tmp"));
    let mut transaction = InstallTransaction {
        target_exe: target_exe.to_path_buf(),
        updater_exe,
        backup_dir,
        backup_app,
        backup_updater,
        new_app,
        new_updater,
        had_updater: install_dir.join(UPDATER_FILE).is_file(),
        new_app_created: false,
        new_updater_created: false,
        app_backed_up: false,
        updater_backed_up: false,
        app_installed: false,
        updater_installed: false,
    };

    let result = (|| {
        copy_and_sync(&stage_dir.join(APP_FILE), &transaction.new_app)?;
        transaction.new_app_created = true;
        copy_and_sync(&stage_dir.join(UPDATER_FILE), &transaction.new_updater)?;
        transaction.new_updater_created = true;

        fs::rename(&transaction.target_exe, &transaction.backup_app)
            .context("move current executable to rollback directory")?;
        transaction.app_backed_up = true;

        if transaction.had_updater {
            fs::rename(&transaction.updater_exe, &transaction.backup_updater)
                .context("move current updater to rollback directory")?;
            transaction.updater_backed_up = true;
        }

        fs::rename(&transaction.new_app, &transaction.target_exe)
            .context("install updated FileManager.exe")?;
        transaction.app_installed = true;
        fs::rename(&transaction.new_updater, &transaction.updater_exe)
            .context("install updated FileManagerUpdater.exe")?;
        transaction.updater_installed = true;
        Ok::<_, anyhow::Error>(())
    })();

    if let Err(error) = result {
        if let Err(rollback_error) = rollback(&transaction) {
            return Err(anyhow::anyhow!(
                "update failed ({error}); rollback failed ({rollback_error}); backup: {}",
                transaction.backup_dir.display()
            ));
        }
        relaunch_previous(target_exe)?;
        return Err(error);
    }
    Ok(transaction)
}

fn copy_and_sync(source: &Path, destination: &Path) -> anyhow::Result<()> {
    let mut input =
        File::open(source).with_context(|| format!("open staged file {}", source.display()))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .with_context(|| format!("create {}", destination.display()))?;
    let result = (|| {
        std::io::copy(&mut input, &mut output).context("copy staged executable")?;
        output.sync_all().context("flush staged executable")?;
        Ok::<_, anyhow::Error>(())
    })();
    drop(output);
    if let Err(error) = result {
        let _ = fs::remove_file(destination);
        return Err(error);
    }
    Ok(())
}

fn rollback(transaction: &InstallTransaction) -> anyhow::Result<()> {
    let mut failures = Vec::new();
    if transaction.updater_installed {
        if let Err(error) = remove_file_if_present(&transaction.updater_exe) {
            failures.push(format!("remove updated updater: {error}"));
        }
    }
    if transaction.app_installed {
        if let Err(error) = remove_file_if_present(&transaction.target_exe) {
            failures.push(format!("remove updated executable: {error}"));
        }
    }
    if transaction.updater_backed_up {
        if let Err(error) = fs::rename(&transaction.backup_updater, &transaction.updater_exe) {
            failures.push(format!("restore previous updater: {error}"));
        }
    }
    if transaction.app_backed_up {
        if let Err(error) = fs::rename(&transaction.backup_app, &transaction.target_exe) {
            failures.push(format!("restore previous executable: {error}"));
        }
    }
    if transaction.new_app_created {
        let _ = remove_file_if_present(&transaction.new_app);
    }
    if transaction.new_updater_created {
        let _ = remove_file_if_present(&transaction.new_updater);
    }
    if failures.is_empty() {
        let _ = fs::remove_file(transaction.backup_dir.join(BACKUP_MARKER));
        let _ = fs::remove_dir(&transaction.backup_dir);
        Ok(())
    } else {
        bail!(
            "rollback could not restore all files; backup retained at {}: {}",
            transaction.backup_dir.display(),
            failures.join("; ")
        )
    }
}

fn remove_file_if_present(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn wait_for_startup(child: &mut Child, marker: &Path) -> anyhow::Result<()> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if marker.is_file() {
            return Ok(());
        }
        if let Some(status) = child.try_wait().context("check updated process startup")? {
            bail!("updated FileManager exited before its first frame ({status})");
        }
        if Instant::now() >= deadline {
            bail!("updated FileManager did not finish startup in time");
        }
        thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("filemanager-{label}-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn updater_arguments_preserve_paths_with_spaces() {
        let args = vec![
            "--pid".into(),
            "42".into(),
            "--target".into(),
            r"C:\Program Files\FileManager\filemanager.exe".into(),
            "--stage".into(),
            r"C:\Users\test user\AppData\Local\Temp\FileManager-update-stage-x".into(),
        ];
        let parsed = parse_args(&args).unwrap();
        assert_eq!(parsed.process_id, 42);
        assert_eq!(
            parsed.target_exe,
            PathBuf::from(r"C:\Program Files\FileManager\filemanager.exe")
        );
        assert!(parsed.stage_dir.ends_with("FileManager-update-stage-x"));
    }

    #[test]
    fn rollback_restores_both_previous_binaries_when_update_cannot_start() {
        let root = temp_dir("updater-rollback");
        let install_dir = root.join("install");
        fs::create_dir(&install_dir).unwrap();
        let stage_dir = std::env::temp_dir().join(format!(
            "FileManager-update-stage-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&stage_dir).unwrap();

        let target = install_dir.join("FileManager-v0.6.5-x64.exe");
        let updater = install_dir.join(UPDATER_FILE);
        let settings = install_dir.join("settings.json");
        fs::write(&target, b"old app").unwrap();
        fs::write(&updater, b"old updater").unwrap();
        fs::write(&settings, b"saved settings").unwrap();
        fs::write(stage_dir.join(APP_FILE), b"new app").unwrap();
        fs::write(stage_dir.join(UPDATER_FILE), b"new updater").unwrap();

        let transaction = install_binaries(&target, &stage_dir).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new app");
        assert_eq!(fs::read(&updater).unwrap(), b"new updater");
        assert_eq!(fs::read(&settings).unwrap(), b"saved settings");
        rollback(&transaction).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"old app");
        assert_eq!(fs::read(&updater).unwrap(), b"old updater");
        assert_eq!(fs::read(&settings).unwrap(), b"saved settings");

        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(stage_dir).unwrap();
    }

    #[test]
    fn rollback_removes_new_updater_if_install_had_no_sidecar() {
        let root = temp_dir("updater-bootstrap");
        let install_dir = root.join("install");
        fs::create_dir(&install_dir).unwrap();
        let stage_dir = std::env::temp_dir().join(format!(
            "FileManager-update-stage-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&stage_dir).unwrap();

        let target = install_dir.join("filemanager.exe");
        fs::write(&target, b"old app").unwrap();
        fs::write(stage_dir.join(APP_FILE), b"new app").unwrap();
        fs::write(stage_dir.join(UPDATER_FILE), b"new updater").unwrap();

        let transaction = install_binaries(&target, &stage_dir).unwrap();
        let updater = install_dir.join(UPDATER_FILE);
        assert!(updater.is_file());
        rollback(&transaction).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"old app");
        assert!(!updater.exists());

        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(stage_dir).unwrap();
    }

    #[test]
    fn new_install_preserves_marked_backup_from_previous_failed_rollback() {
        let root = temp_dir("updater-preserve-backup");
        let install_dir = root.join("install");
        fs::create_dir(&install_dir).unwrap();
        let stage_dir = std::env::temp_dir().join(format!(
            "FileManager-update-stage-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&stage_dir).unwrap();

        let target = install_dir.join(APP_FILE);
        let updater = install_dir.join(UPDATER_FILE);
        fs::write(&target, b"old app").unwrap();
        fs::write(&updater, b"old updater").unwrap();
        fs::write(stage_dir.join(APP_FILE), b"new app").unwrap();
        fs::write(stage_dir.join(UPDATER_FILE), b"new updater").unwrap();

        let previous_backup = install_dir.join(format!("{BACKUP_PREFIX}previous"));
        fs::create_dir(&previous_backup).unwrap();
        fs::write(previous_backup.join(BACKUP_MARKER), b"").unwrap();
        fs::write(previous_backup.join(APP_FILE), b"last recoverable app").unwrap();

        let transaction = install_binaries(&target, &stage_dir).unwrap();
        assert_eq!(
            fs::read(previous_backup.join(APP_FILE)).unwrap(),
            b"last recoverable app"
        );
        assert!(previous_backup.join(BACKUP_MARKER).is_file());

        rollback(&transaction).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"old app");
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(stage_dir).unwrap();
    }
}
