//! Создание, перечисление и удаление ярлыков.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::desktop::{self, DesktopEntry};
use crate::error::{Error, Result};
use crate::exec;

#[derive(Debug, Clone, Default)]
pub struct ShortcutSpec {
    pub name: String,
    pub path: String,
    pub icon: Option<String>,
    pub workdir: Option<String>,
    pub args: Option<String>,
    pub terminal: bool,
}

/// Проверенный и нормализованный ярлык.
#[derive(Debug, Clone)]
pub struct PreparedShortcut {
    pub name: String,
    pub exec: Vec<String>,
    pub icon: Option<String>,
    pub workdir: Option<String>,
    pub terminal: bool,
}

impl PreparedShortcut {
    pub fn file_name(&self) -> String {
        format!("{}.desktop", slugify(&self.name))
    }

    pub fn desktop_entry(&self) -> DesktopEntry {
        DesktopEntry {
            name: self.name.clone(),
            exec: self.exec.clone(),
            icon: self.icon.clone(),
            workdir: self.workdir.clone(),
            terminal: self.terminal,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ShortcutInfo {
    pub name: String,
    pub exec: String,
    pub path: PathBuf,
}

/// `~/.local/share/applications` с поддержкой XDG_DATA_HOME.
pub fn applications_dir() -> Result<PathBuf> {
    if let Some(data_home) = env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(data_home).join("applications"));
    }

    let home = env::var_os("HOME").ok_or_else(|| {
        Error::message(
            "переменная HOME не задана; не удалось определить каталог ~/.local/share/applications",
        )
    })?;
    Ok(PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("applications"))
}

pub fn prepare(spec: &ShortcutSpec, cwd: &Path) -> Result<PreparedShortcut> {
    let name = spec.name.trim();
    if name.is_empty() {
        return Err(Error::usage("название ярлыка не может быть пустым"));
    }
    if name.chars().any(char::is_control) {
        return Err(Error::usage(
            "название ярлыка не должно содержать управляющие символы",
        ));
    }

    let mut command = exec::resolve(&spec.path, cwd)?;
    if let Some(raw_args) = spec.args.as_deref() {
        if !raw_args.trim().is_empty() {
            command.append(&mut exec::tokenize(raw_args)?);
        }
    }

    let icon = spec
        .icon
        .as_deref()
        .map(|value| exec::resolve_file(value, cwd, "иконка"))
        .transpose()?
        .map(|path| path.to_string_lossy().into_owned());

    let workdir = spec
        .workdir
        .as_deref()
        .map(|value| exec::resolve_directory(value, cwd))
        .transpose()?
        .map(|path| path.to_string_lossy().into_owned());

    Ok(PreparedShortcut {
        name: name.to_string(),
        exec: command,
        icon,
        workdir,
        terminal: spec.terminal,
    })
}

/// Записывает ярлык атомарно. Если файл существует и `force == false`,
/// возвращает `Error::AlreadyExists`.
pub fn create(shortcut: &PreparedShortcut, apps_dir: &Path, force: bool) -> Result<PathBuf> {
    fs::create_dir_all(apps_dir).map_err(|error| {
        Error::io(
            format!("не удалось создать каталог {}", apps_dir.display()),
            error,
        )
    })?;

    let target = apps_dir.join(shortcut.file_name());

    // Перезаписываем только обычные файлы, созданные этой утилитой.
    // Симлинки, каталоги и чужие .desktop-файлы не трогаем даже с --force.
    match fs::symlink_metadata(&target) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() || !is_owned_shortcut(&target) {
                return Err(Error::ForeignShortcut(target));
            }
            if !force {
                return Err(Error::AlreadyExists(target));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(Error::io(
                format!("не удалось проверить {}", target.display()),
                error,
            ));
        }
    }

    let content = shortcut.desktop_entry().render();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temporary = apps_dir.join(format!(
        ".desklink-{}-{nonce}.desktop.tmp",
        std::process::id()
    ));

    fs::write(&temporary, content).map_err(|error| {
        Error::io(
            format!("не удалось записать временный файл {}", temporary.display()),
            error,
        )
    })?;

    if let Err(error) = fs::rename(&temporary, &target) {
        let _ = fs::remove_file(&temporary);
        return Err(Error::io(
            format!("не удалось сохранить ярлык {}", target.display()),
            error,
        ));
    }

    Ok(target)
}

fn is_owned_shortcut(path: &Path) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|content| parse_desktop(&content))
        .is_some_and(|parsed| parsed.created_by_label)
}

/// Возвращает только ярлыки с маркером `X-Label-CLI=true`.
pub fn list(apps_dir: &Path) -> Result<Vec<ShortcutInfo>> {
    let entries = match fs::read_dir(apps_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(Error::io(
                format!("не удалось прочитать каталог {}", apps_dir.display()),
                error,
            ));
        }
    };

    let mut shortcuts = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            Error::io(
                format!("не удалось прочитать содержимое {}", apps_dir.display()),
                error,
            )
        })?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("desktop") {
            continue;
        }

        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let Some(parsed) = parse_desktop(&content) else {
            continue;
        };
        if !parsed.created_by_label {
            continue;
        }

        let fallback_name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("Без имени")
            .to_string();

        shortcuts.push(ShortcutInfo {
            name: parsed.name.unwrap_or(fallback_name),
            exec: parsed.exec.unwrap_or_default(),
            path,
        });
    }

    shortcuts.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(shortcuts)
}

/// Удаляет ярлык по отображаемому имени, имени файла или полному пути.
/// Удаляются только файлы, созданные этой утилитой.
pub fn remove(apps_dir: &Path, query: &str) -> Result<Vec<PathBuf>> {
    let query = query.trim();
    if query.is_empty() {
        return Err(Error::usage("не указано название ярлыка для удаления"));
    }

    let shortcuts = list(apps_dir)?;
    let mut matches = shortcuts
        .iter()
        .filter(|shortcut| shortcut.name.eq_ignore_ascii_case(query))
        .collect::<Vec<_>>();

    if matches.is_empty() {
        let canonical_query = fs::canonicalize(query).ok();
        matches = shortcuts
            .iter()
            .filter(|shortcut| {
                let file_name_matches = shortcut
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.eq_ignore_ascii_case(query));
                let stem_matches = shortcut
                    .path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|stem| stem.eq_ignore_ascii_case(query));
                let path_matches = canonical_query
                    .as_ref()
                    .is_some_and(|path| path == &shortcut.path);
                file_name_matches || stem_matches || path_matches
            })
            .collect();
    }

    if matches.is_empty() {
        return Err(Error::message(format!(
            "ярлык «{query}» не найден среди ярлыков, созданных desklink"
        )));
    }

    let mut removed = Vec::new();
    for shortcut in matches {
        fs::remove_file(&shortcut.path).map_err(|error| {
            Error::io(
                format!("не удалось удалить {}", shortcut.path.display()),
                error,
            )
        })?;
        removed.push(shortcut.path.clone());
    }
    Ok(removed)
}

/// Безопасное имя файла: Unicode-буквы и цифры сохраняются, остальные
/// символы превращаются в одиночный дефис.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut last_was_dash = false;

    for ch in name.trim().chars() {
        if ch.is_alphanumeric() || ch == '_' {
            out.extend(ch.to_lowercase());
            last_was_dash = false;
        } else if !out.is_empty() && !last_was_dash {
            out.push('-');
            last_was_dash = true;
        }
    }

    while out.ends_with('-') {
        out.pop();
    }

    let out = out.chars().take(80).collect::<String>();
    let out = out.trim_matches('-');

    if out.is_empty() {
        "shortcut".to_string()
    } else {
        out.to_string()
    }
}

#[derive(Debug, Default)]
struct ParsedDesktop {
    name: Option<String>,
    exec: Option<String>,
    created_by_label: bool,
}

fn parse_desktop(content: &str) -> Option<ParsedDesktop> {
    let mut in_desktop_entry = false;
    let mut saw_desktop_entry = false;
    let mut parsed = ParsedDesktop::default();

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            let section = &line[1..line.len() - 1];
            in_desktop_entry = section == "Desktop Entry";
            saw_desktop_entry |= in_desktop_entry;
            continue;
        }

        if !in_desktop_entry {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        match key.trim() {
            "Name" => parsed.name = Some(desktop::unescape_value(value.trim())),
            "Exec" => parsed.exec = Some(value.trim().to_string()),
            "X-Label-CLI" => {
                parsed.created_by_label = value.trim().eq_ignore_ascii_case("true");
            }
            _ => {}
        }
    }

    saw_desktop_entry.then_some(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = env::temp_dir().join(format!(
                "desklink-shortcut-{tag}-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn make_executable(path: &Path) {
        fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    fn spec(app: &Path, name: &str) -> ShortcutSpec {
        ShortcutSpec {
            name: name.to_string(),
            path: app.to_string_lossy().into_owned(),
            ..ShortcutSpec::default()
        }
    }

    #[test]
    fn slugifies_names_safely() {
        assert_eq!(slugify("Test App"), "test-app");
        assert_eq!(slugify("  Мой Ярлык  "), "мой-ярлык");
        assert_eq!(slugify("a/b\\c"), "a-b-c");
        assert_eq!(slugify("***"), "shortcut");
        assert_eq!(slugify(""), "shortcut");
    }

    #[test]
    fn creates_lists_and_removes_shortcut() {
        let temp = TempDir::new("roundtrip");
        let app = temp.path().join("app");
        let apps = temp.path().join("applications");
        make_executable(&app);

        let prepared = prepare(&spec(&app, "Test App"), temp.path()).unwrap();
        let target = create(&prepared, &apps, false).unwrap();
        assert_eq!(target.file_name().unwrap(), "test-app.desktop");

        let listed = list(&apps).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Test App");
        assert!(listed[0].exec.contains("app"));

        let removed = remove(&apps, "Test App").unwrap();
        assert_eq!(removed, vec![target.clone()]);
        assert!(!target.exists());
        assert!(list(&apps).unwrap().is_empty());
    }

    #[test]
    fn refuses_to_overwrite_without_force() {
        let temp = TempDir::new("overwrite");
        let app = temp.path().join("app");
        let apps = temp.path().join("applications");
        make_executable(&app);

        let prepared = prepare(&spec(&app, "Test App"), temp.path()).unwrap();
        create(&prepared, &apps, false).unwrap();

        let error = create(&prepared, &apps, false).unwrap_err();
        assert!(matches!(error, Error::AlreadyExists(_)));

        create(&prepared, &apps, true).unwrap();
    }

    #[test]
    fn refuses_to_overwrite_foreign_file_even_with_force() {
        let temp = TempDir::new("foreign-force");
        let app = temp.path().join("app");
        let apps = temp.path().join("applications");
        let target = apps.join("test-app.desktop");
        make_executable(&app);
        fs::create_dir_all(&apps).unwrap();

        let foreign = "[Desktop Entry]\nType=Application\nName=Foreign\nExec=/bin/true\n";
        fs::write(&target, foreign).unwrap();

        let prepared = prepare(&spec(&app, "Test App"), temp.path()).unwrap();

        let error = create(&prepared, &apps, false).unwrap_err();
        assert!(matches!(error, Error::ForeignShortcut(_)));
        let error = create(&prepared, &apps, true).unwrap_err();
        assert!(matches!(error, Error::ForeignShortcut(_)));
        assert_eq!(fs::read_to_string(&target).unwrap(), foreign);
    }

    #[test]
    fn refuses_to_replace_symlink_even_with_force() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new("symlink");
        let app = temp.path().join("app");
        let apps = temp.path().join("applications");
        let foreign = apps.join("foreign.desktop");
        let target = apps.join("test-app.desktop");
        make_executable(&app);
        fs::create_dir_all(&apps).unwrap();

        fs::write(&foreign, "user data\n").unwrap();
        symlink("foreign.desktop", &target).unwrap();

        let prepared = prepare(&spec(&app, "Test App"), temp.path()).unwrap();
        let error = create(&prepared, &apps, true).unwrap_err();

        assert!(matches!(error, Error::ForeignShortcut(_)));
        assert!(
            fs::symlink_metadata(&target)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&foreign).unwrap(), "user data\n");
    }

    #[test]
    fn ignores_foreign_desktop_files() {
        let temp = TempDir::new("foreign");
        let apps = temp.path().join("applications");
        fs::create_dir_all(&apps).unwrap();
        fs::write(
            apps.join("foreign.desktop"),
            "[Desktop Entry]\nType=Application\nName=Foreign\nExec=/bin/true\n",
        )
        .unwrap();

        assert!(list(&apps).unwrap().is_empty());
        let error = remove(&apps, "Foreign").unwrap_err();
        assert!(error.to_string().contains("не найден"));
    }

    #[test]
    fn lists_missing_directory_as_empty() {
        let temp = TempDir::new("missing-dir");
        assert!(list(&temp.path().join("nope")).unwrap().is_empty());
    }

    #[test]
    fn includes_icon_workdir_args_and_terminal() {
        let temp = TempDir::new("fields");
        let app = temp.path().join("app");
        let icon = temp.path().join("icon.png");
        let workdir = temp.path().join("work");
        let apps = temp.path().join("applications");
        make_executable(&app);
        fs::write(&icon, "png").unwrap();
        fs::create_dir(&workdir).unwrap();

        let mut input = spec(&app, "Fields");
        input.icon = Some("./icon.png".into());
        input.workdir = Some("./work".into());
        input.args = Some("--foo \"bar baz\"".into());
        input.terminal = true;

        let prepared = prepare(&input, temp.path()).unwrap();
        let target = create(&prepared, &apps, false).unwrap();
        let content = fs::read_to_string(target).unwrap();

        assert!(content.contains(&format!("Icon={}", icon.canonicalize().unwrap().display())));
        assert!(content.contains(&format!(
            "Path={}",
            workdir.canonicalize().unwrap().display()
        )));
        assert!(content.contains("--foo \"bar baz\""));
        assert!(content.contains("Terminal=true"));
    }

    #[test]
    fn parses_localized_unescaped_name() {
        let parsed = parse_desktop(
            "[Desktop Entry]\nName=My\\sApp\\nLine\nExec=/bin/true\nX-Label-CLI=true\n",
        )
        .unwrap();
        assert_eq!(parsed.name.unwrap(), "My App\nLine");
        assert!(parsed.created_by_label);
    }
}
