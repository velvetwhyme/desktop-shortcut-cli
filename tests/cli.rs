use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestEnv {
    root: PathBuf,
    home: PathBuf,
    data: PathBuf,
    bin: PathBuf,
}

impl TestEnv {
    fn new(tag: &str) -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "desklink-it-{tag}-{}-{unique}-{timestamp}",
            std::process::id()
        ));
        let home = root.join("home");
        let data = root.join("xdg-data");
        let bin = root.join("bin");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&bin).unwrap();
        Self {
            root,
            home,
            data,
            bin,
        }
    }

    fn apps_dir(&self) -> PathBuf {
        self.data.join("applications")
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_desklink"));
        command
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .env("XDG_DATA_HOME", &self.data)
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", self.bin.display()),
            )
            .stdin(Stdio::null());
        command
    }

    fn run(&self, arguments: &[&str]) -> Output {
        self.command().args(arguments).output().unwrap()
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn make_executable(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn creates_lists_and_removes_shortcut_with_spaces() {
    let env = TestEnv::new("roundtrip");
    let app = env.root.join("My Apps").join("test app.sh");
    make_executable(&app, "#!/bin/sh\nexit 0\n");

    let output = env.run(&[
        "-n",
        "Test App",
        "-p",
        "./My Apps/test app.sh",
        "-a",
        "--fullscreen",
        "-t",
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let shortcut = env.apps_dir().join("test-app.desktop");
    assert!(shortcut.is_file());

    let content = fs::read_to_string(&shortcut).unwrap();
    let absolute_app = app.canonicalize().unwrap();
    assert!(content.contains("Name=Test App\n"));
    assert!(content.contains(&format!(
        "Exec=\"{}\" --fullscreen\n",
        absolute_app.display()
    )));
    assert!(content.contains("Terminal=true\n"));
    assert!(content.contains("X-Label-CLI=true\n"));

    let listed = env.run(&["--list"]);
    assert!(listed.status.success(), "stderr: {}", stderr(&listed));
    let listed = stdout(&listed);
    assert!(listed.contains("Test App"));
    assert!(listed.contains("test-app.desktop"));

    let removed = env.run(&["--remove", "Test App"]);
    assert!(removed.status.success(), "stderr: {}", stderr(&removed));
    assert!(stdout(&removed).contains("Удалён"));
    assert!(!shortcut.exists());
}

#[test]
fn refuses_overwrite_without_force_and_allows_it_with_force() {
    let env = TestEnv::new("overwrite");
    let app = env.root.join("app");
    make_executable(&app, "#!/bin/sh\nexit 0\n");

    let first = env.run(&["-n", "Test App", "-p", "./app"]);
    assert!(first.status.success(), "stderr: {}", stderr(&first));

    let second = env.run(&["-n", "Test App", "-p", "./app"]);
    assert_eq!(second.status.code(), Some(1));
    let message = stderr(&second);
    assert!(message.contains("уже существует"));
    assert!(message.contains("--force"));

    let forced = env.run(&["-n", "Test App", "-p", "./app", "--force"]);
    assert!(forced.status.success(), "stderr: {}", stderr(&forced));
}

#[test]
fn launches_windows_exe_through_wine() {
    let env = TestEnv::new("wine");
    let wine = env.bin.join("wine");
    make_executable(&wine, "#!/bin/sh\nexec \"$@\"\n");
    let game = env.root.join("game.exe");
    fs::write(&game, b"MZ fake").unwrap();

    let output = env.run(&["-n", "Game", "-p", "./game.exe"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let content = fs::read_to_string(env.apps_dir().join("game.desktop")).unwrap();
    assert!(content.contains(&format!(
        "Exec={} {}\n",
        wine.canonicalize().unwrap().display(),
        game.canonicalize().unwrap().display()
    )));
}

#[test]
fn resolves_command_from_path_and_escapes_arguments() {
    let env = TestEnv::new("path-command");
    let tool = env.bin.join("demo-tool");
    make_executable(&tool, "#!/bin/sh\nexit 0\n");

    let output = env.run(&["-n", "Shell", "-p", "demo-tool", "-a", "-c 'echo hi'"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let content = fs::read_to_string(env.apps_dir().join("shell.desktop")).unwrap();
    assert!(content.contains(&format!(
        "Exec={} -c \"echo hi\"\n",
        tool.canonicalize().unwrap().display()
    )));
}

#[test]
fn refuses_to_overwrite_foreign_desktop_even_with_force() {
    let env = TestEnv::new("foreign-force");
    let apps = env.apps_dir();
    fs::create_dir_all(&apps).unwrap();
    let target = apps.join("test-app.desktop");
    let foreign = "[Desktop Entry]\nType=Application\nName=Foreign App\nExec=/bin/true\nComment=DO NOT REPLACE\n";
    fs::write(&target, foreign).unwrap();

    let output = env.run(&["-n", "Test App", "-p", "/bin/true", "--force"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("не создан desklink"));
    assert_eq!(fs::read_to_string(&target).unwrap(), foreign);
}

#[test]
fn explicit_launcher_absolutizes_relative_exe() {
    let env = TestEnv::new("alt-launcher");
    let launcher = env.bin.join("alt-run");
    make_executable(&launcher, "#!/bin/sh\nexit 0\n");
    let game = env.root.join("Games").join("My Game").join("game.exe");
    fs::create_dir_all(game.parent().unwrap()).unwrap();
    fs::write(&game, "MZ safe stub\n").unwrap();
    fs::create_dir_all(env.root.join("Work Dir")).unwrap();

    let output = env.run(&[
        "-n",
        "Alt Game",
        "-p",
        "alt-run \"./Games/My Game/game.exe\"",
        "-w",
        "./Work Dir",
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let content = fs::read_to_string(env.apps_dir().join("alt-game.desktop")).unwrap();
    assert!(content.contains(&format!(
        "Exec={} \"{}\"\n",
        launcher.canonicalize().unwrap().display(),
        game.canonicalize().unwrap().display()
    )));
    assert!(!content.contains("./Games/My Game/game.exe"));
}

#[test]
fn manages_legacy_label_cli_shortcuts() {
    let env = TestEnv::new("legacy");
    let apps = env.apps_dir();
    fs::create_dir_all(&apps).unwrap();
    let target = apps.join("legacy-app.desktop");
    let legacy = "[Desktop Entry]\nType=Application\nVersion=1.5\nName=Legacy App\nExec=/bin/true\nComment=Shortcut created by label-cli\nTerminal=false\nCategories=Utility;\nX-Label-CLI=true\n";
    fs::write(&target, legacy).unwrap();

    let listed = env.run(&["--list"]);
    assert!(listed.status.success(), "stderr: {}", stderr(&listed));
    let listed = stdout(&listed);
    assert!(listed.contains("Legacy App"));
    assert!(listed.contains("legacy-app.desktop"));

    let app = env.root.join("app");
    make_executable(&app, "#!/bin/sh\nexit 0\n");
    let forced = env.run(&["-n", "Legacy App", "-p", "./app", "--force"]);
    assert!(forced.status.success(), "stderr: {}", stderr(&forced));
    let updated = fs::read_to_string(&target).unwrap();
    assert!(updated.contains("X-Label-CLI=true\n"));
    assert!(!updated.contains("Comment=Shortcut created by label-cli"));

    let removed = env.run(&["--remove", "Legacy App"]);
    assert!(removed.status.success(), "stderr: {}", stderr(&removed));
    assert!(!target.exists());
}

#[test]
fn rejects_grouped_short_flags_and_missing_values() {
    let env = TestEnv::new("bad-args");

    let grouped = env.run(&["-np", "Test App"]);
    assert_eq!(grouped.status.code(), Some(2));
    assert!(stderr(&grouped).contains("нельзя объединять"));

    let missing = env.run(&["-n", "Test App"]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(stderr(&missing).contains("не хватает обязательных параметров"));
}

#[test]
fn help_and_empty_list_work_without_side_effects() {
    let env = TestEnv::new("help");

    let help = env.run(&["--help"]);
    assert!(help.status.success());
    assert!(stdout(&help).contains("Использование:"));
    assert!(stdout(&help).contains("--remove"));

    let list = env.run(&["--list"]);
    assert!(list.status.success());
    assert!(stdout(&list).contains("не найдено"));
    assert!(!env.apps_dir().exists());
}
