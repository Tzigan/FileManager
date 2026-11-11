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
    /// false — файлы можно заменить, но новый процесс не запускать.
    auto_restart: bool,
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
        // Ошибка замены: новую версию не запускаем. install_binaries уже вернул старые файлы.
        Err(error) => return Err(error),
    };

    // Вариант A. Старый процесс уже завершился, файлы лежат по целевому пути.
    // Запуск только отсюда: не из основного окна и не из временного файла .tmp.
    if restart_plan(true, args.auto_restart) == RestartPlan::DoNotStart {
        let _ = fs::remove_dir_all(&transaction.backup_dir);
        return Ok(());
    }

    let mut child =
        match updated_app_command(&transaction.target_exe, &args.stage_dir, std::process::id())
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

/// Что делать после попытки установки. Новую версию запускаем только при успехе и включённом флаге.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestartPlan {
    StartUpdated,
    DoNotStart,
}

fn restart_plan(install_succeeded: bool, auto_restart: bool) -> RestartPlan {
    if install_succeeded && auto_restart {
        RestartPlan::StartUpdated
    } else {
        RestartPlan::DoNotStart
    }
}

/// Команда запуска уже установленного exe. Путь — целевой, не временная копия в каталоге установки.
fn updated_app_command(target_exe: &Path, stage_dir: &Path, updater_pid: u32) -> Command {
    let mut command = Command::new(target_exe);
    command
        .env(UPDATE_STAGE_ENV, stage_dir)
        .env(UPDATE_UPDATER_PID_ENV, updater_pid.to_string());
    command
}

fn relaunch_previous(target_exe: &Path) -> anyhow::Result<()> {
    Command::new(target_exe)
        .spawn()
        .context("relaunch previous FileManager after rollback")?;
    Ok(())
}

/// Короткий текст для MessageBox. Без цепочки anyhow и без путей из стека ошибки.
pub fn user_facing_message(error: &anyhow::Error) -> String {
    message_for(error, interface_is_russian())
}

fn message_for(error: &anyhow::Error, russian: bool) -> String {
    if error_is_file_busy(error) {
        return if russian {
            "Файл занят. Закройте программу и повторите установку.".to_owned()
        } else {
            "The file is in use. Close FileManager and try the update again.".to_owned()
        };
    }
    if chain_has(error, "launch updated FileManager")
        || chain_has(error, "did not finish startup")
        || chain_has(error, "exited before its first frame")
    {
        return if russian {
            "Не удалось запустить новую версию. Предыдущая версия восстановлена.".to_owned()
        } else {
            "The new version could not be started. The previous version was restored.".to_owned()
        };
    }
    if chain_has(error, "executable is missing") || chain_has(error, "not a FileManager") {
        return if russian {
            "Не найден файл программы для запуска.".to_owned()
        } else {
            "The application executable to start was not found.".to_owned()
        };
    }
    if russian {
        "Не удалось установить обновление. Новая версия не запущена.".to_owned()
    } else {
        "The update was not installed. The new version was not started.".to_owned()
    }
}

fn error_is_file_busy(error: &anyhow::Error) -> bool {
    // Занятый exe на Windows чаще всего даёт код 5, иногда 32 или 33.
    error.chain().any(|item| {
        item.downcast_ref::<std::io::Error>()
            .and_then(|io| io.raw_os_error())
            .is_some_and(|code| matches!(code, 5 | 32 | 33))
    })
}

fn chain_has(error: &anyhow::Error, needle: &str) -> bool {
    error.chain().any(|item| item.to_string().contains(needle))
}

fn interface_is_russian() -> bool {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--lang" {
            return args
                .get(index + 1)
                .is_some_and(|value| value.eq_ignore_ascii_case("ru"));
        }
        index += 1;
    }
    true
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
    let mut auto_restart = true;
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
            "--restart" => {
                auto_restart = match value.to_string_lossy().as_ref() {
                    "1" | "true" | "yes" => true,
                    "0" | "false" | "no" => false,
                    other => bail!("invalid --restart value: {other}"),
                };
            }
            // Язык окна ошибки. На решение «запускать или нет» не влияет.
            "--lang" => {}
            _ => bail!("unknown updater argument: {name}"),
        }
        index += 2;
    }
    Ok(UpdaterArgs {
        process_id: process_id.context("missing --pid")?,
        target_exe: target_exe.context("missing --target")?,
        stage_dir: stage_dir.context("missing --stage")?,
        auto_restart,
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

    #[test]
    fn successful_install_plans_to_start_the_updated_executable() {
        assert_eq!(restart_plan(true, true), RestartPlan::StartUpdated);
        let command = updated_app_command(
            Path::new(r"C:\Program Files\FileManager\FileManager.exe"),
            Path::new(r"C:\Temp\FileManager-update-stage-1"),
            9,
        );
        assert_eq!(
            command.get_program(),
            Path::new(r"C:\Program Files\FileManager\FileManager.exe")
        );
        assert!(command.get_envs().any(|(key, value)| {
            key == UPDATE_STAGE_ENV
                && value.is_some_and(|value| value.to_string_lossy().contains("update-stage"))
        }));
    }

    #[test]
    fn failed_install_does_not_plan_a_restart() {
        assert_eq!(restart_plan(false, true), RestartPlan::DoNotStart);
        assert_eq!(restart_plan(false, false), RestartPlan::DoNotStart);
    }

    #[test]
    fn auto_restart_flag_is_respected() {
        assert_eq!(restart_plan(true, false), RestartPlan::DoNotStart);
        let disabled = parse_args(&[
            "--pid".into(),
            "7".into(),
            "--target".into(),
            r"C:\Apps\FileManager.exe".into(),
            "--stage".into(),
            r"C:\Temp\FileManager-update-stage-x".into(),
            "--restart".into(),
            "0".into(),
            "--lang".into(),
            "en".into(),
        ])
        .unwrap();
        assert!(!disabled.auto_restart);
        assert_eq!(
            restart_plan(true, disabled.auto_restart),
            RestartPlan::DoNotStart
        );
    }

    #[test]
    fn busy_file_has_a_plain_message_and_does_not_restart() {
        let error = anyhow::Error::new(std::io::Error::from_raw_os_error(32))
            .context("move current executable to rollback directory");
        let text = message_for(&error, true);
        assert!(text.contains("Файл занят"));
        assert!(!text.contains("rollback"));
        assert_eq!(restart_plan(false, true), RestartPlan::DoNotStart);

        let denied = anyhow::Error::new(std::io::Error::from_raw_os_error(5));
        assert!(message_for(&denied, false).contains("file is in use"));

        let launch = anyhow::Error::msg("open").context("launch updated FileManager");
        assert!(message_for(&launch, true).contains("Не удалось запустить новую версию"));
    }
}
