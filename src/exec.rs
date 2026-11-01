//! Разбор команды запуска, поиск в PATH и проверка путей.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Разбирает строку аргументов как shell-подобную последовательность.
/// Поддерживаются одинарные и двойные кавычки и экранирование через `\`.
pub fn tokenize(input: &str) -> Result<Vec<String>> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Normal,
        Single,
        Double,
    }

    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut state = State::Normal;
    let mut chars = input.chars();

    while let Some(ch) = chars.next() {
        match state {
            State::Normal => match ch {
                '\'' => {
                    state = State::Single;
                    started = true;
                }
                '"' => {
                    state = State::Double;
                    started = true;
                }
                '\\' => {
                    let Some(next) = chars.next() else {
                        return Err(Error::usage(
                            "в аргументах указан одиночный обратный слэш в конце строки",
                        ));
                    };
                    current.push(next);
                    started = true;
                }
                c if c.is_whitespace() => {
                    if started {
                        tokens.push(std::mem::take(&mut current));
                        started = false;
                    }
                }
                c => {
                    current.push(c);
                    started = true;
                }
            },
            State::Single => {
                if ch == '\'' {
                    state = State::Normal;
                } else {
                    current.push(ch);
                }
            }
            State::Double => match ch {
                '"' => state = State::Normal,
                '\\' => {
                    let Some(next) = chars.next() else {
                        return Err(Error::usage("незавершённое экранирование в аргументах"));
                    };
                    // Внутри двойных кавычек \ экранирует только эти символы.
                    if matches!(next, '"' | '\\' | '$' | '`') {
                        current.push(next);
                    } else if next != '\n' {
                        current.push('\\');
                        current.push(next);
                    }
                }
                c => current.push(c),
            },
        }
    }

    if state != State::Normal {
        return Err(Error::usage("незакрытая кавычка в аргументах"));
    }
    if started {
        tokens.push(current);
    }

    Ok(tokens)
}

/// Превращает `-p` в список аргументов, где исполняемый файл имеет
/// абсолютный путь. Для `.exe` автоматически добавляется `wine`, если он
/// доступен; относительные `.exe` в явной команде приводятся к абсолютным.
pub fn resolve(input: &str, cwd: &Path) -> Result<Vec<String>> {
    if input.trim().is_empty() {
        return Err(Error::usage(
            "путь к исполняемому файлу или команда не могут быть пустыми",
        ));
    }

    // Если вся строка — существующий путь, не разбираем её как команду:
    // это важно для путей с пробелами.
    let whole_path = {
        let path = Path::new(input);
        let candidate = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        candidate.is_file().then_some(candidate)
    };

    let (program, mut tokens) = if let Some(candidate) = whole_path {
        (validate_executable(&candidate)?, Vec::new())
    } else {
        let mut tokens = tokenize(input)?;
        if tokens.is_empty() {
            return Err(Error::usage(
                "путь к исполняемому файлу или команда не могут быть пустыми",
            ));
        }

        let first = tokens.remove(0);
        let explicit_path = first.contains('/') || cwd.join(&first).is_file();
        let program = if explicit_path {
            resolve_executable_path(&first, cwd)?
        } else {
            find_in_path(&first, cwd)?
        };
        (program, tokens)
    };

    let mut command = Vec::new();

    if is_windows_exe(&program) {
        let wine = find_optional_in_path("wine", cwd)
            .or_else(|| find_optional_in_path("wine64", cwd))
            .ok_or_else(|| {
                Error::message(format!(
                    "обнаружен Windows-исполняемый файл {}, но wine не найден в PATH.\n\
                     Установите wine или укажите команду запуска явно, например:\n  \
                     desklink -n \"Игра\" -p \"wine {}\"",
                    program.display(),
                    program.display()
                ))
            })?;
        command.push(path_string(&wine));
    }

    command.push(path_string(&program));

    // Для явных launcher-команд (`wine game.exe`, `umu-run game.exe` и т.п.)
    // приводим существующие относительные .exe к абсолютным путям: иначе
    // после установки Path= путь может разрешиться уже в другой директории.
    for argument in &mut tokens {
        let argument_path = Path::new(argument);
        if !is_windows_exe(argument_path) {
            continue;
        }

        let candidate = if argument_path.is_absolute() {
            argument_path.to_path_buf()
        } else {
            cwd.join(argument_path)
        };
        if candidate.is_file() {
            if let Ok(canonical) = candidate.canonicalize() {
                *argument = path_string(&canonical);
            }
        }
    }

    command.append(&mut tokens);
    Ok(command)
}

/// Проверяет существующий файл, делает путь абсолютным и проверяет
/// право на выполнение.
pub fn resolve_file(input: &str, cwd: &Path, kind: &str) -> Result<PathBuf> {
    if input.trim().is_empty() {
        return Err(Error::usage(format!("путь ({kind}) не может быть пустым")));
    }

    let path = Path::new(input);
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };

    let metadata = fs::metadata(&candidate).map_err(|error| {
        Error::message(format!(
            "не удалось найти {kind}: {}\nПричина: {error}",
            candidate.display()
        ))
    })?;

    if !metadata.is_file() {
        return Err(Error::message(format!(
            "{kind} не является обычным файлом: {}",
            candidate.display()
        )));
    }

    candidate.canonicalize().map_err(|error| {
        Error::io(
            format!(
                "не удалось получить абсолютный путь к {}",
                candidate.display()
            ),
            error,
        )
    })
}

/// Проверяет и нормализует рабочую директорию.
pub fn resolve_directory(input: &str, cwd: &Path) -> Result<PathBuf> {
    if input.trim().is_empty() {
        return Err(Error::usage("рабочая директория не может быть пустой"));
    }

    let path = Path::new(input);
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };

    let metadata = fs::metadata(&candidate).map_err(|error| {
        Error::message(format!(
            "не удалось найти рабочую директорию: {}\nПричина: {error}",
            candidate.display()
        ))
    })?;

    if !metadata.is_dir() {
        return Err(Error::message(format!(
            "рабочая директория не является каталогом: {}",
            candidate.display()
        )));
    }

    candidate.canonicalize().map_err(|error| {
        Error::io(
            format!(
                "не удалось получить абсолютный путь к {}",
                candidate.display()
            ),
            error,
        )
    })
}

fn resolve_executable_path(input: &str, cwd: &Path) -> Result<PathBuf> {
    let path = Path::new(input);
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    validate_executable(&candidate)
}

fn validate_executable(candidate: &Path) -> Result<PathBuf> {
    let metadata = fs::metadata(candidate).map_err(|error| {
        Error::message(format!(
            "исполняемый файл не найден: {}\nПричина: {error}",
            candidate.display()
        ))
    })?;

    if !metadata.is_file() {
        return Err(Error::message(format!(
            "указанный путь не является файлом: {}",
            candidate.display()
        )));
    }

    if !is_windows_exe(candidate) && !is_executable(&metadata) {
        return Err(Error::message(format!(
            "файл не имеет права на выполнение: {}\n\
             Добавьте его командой: chmod +x {}",
            candidate.display(),
            candidate.display()
        )));
    }

    candidate.canonicalize().map_err(|error| {
        Error::io(
            format!(
                "не удалось получить абсолютный путь к {}",
                candidate.display()
            ),
            error,
        )
    })
}

fn find_in_path(command: &str, cwd: &Path) -> Result<PathBuf> {
    let path_var = env::var_os("PATH").unwrap_or_default();
    find_in_paths(command, cwd, env::split_paths(&path_var))
}

fn find_optional_in_path(command: &str, cwd: &Path) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    find_in_paths(command, cwd, env::split_paths(&path_var)).ok()
}

fn find_in_paths<I>(command: &str, cwd: &Path, directories: I) -> Result<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    let mut found_without_exec = None;

    for directory in directories {
        let directory = if directory.as_os_str().is_empty() {
            cwd.to_path_buf()
        } else {
            directory
        };
        let candidate = directory.join(command);
        if !candidate.is_file() {
            continue;
        }

        let Ok(canonical) = candidate.canonicalize() else {
            continue;
        };
        let Ok(metadata) = fs::metadata(&canonical) else {
            continue;
        };

        if is_executable(&metadata) {
            return Ok(canonical);
        }
        found_without_exec.get_or_insert(canonical);
    }

    if let Some(path) = found_without_exec {
        return Err(Error::message(format!(
            "команда найдена, но не имеет права на выполнение: {}\n\
             Добавьте его командой: chmod +x {}",
            path.display(),
            path.display()
        )));
    }

    Err(Error::message(format!(
        "команда не найдена в PATH: {command}\n\
         Убедитесь, что она установлена, или укажите полный путь к исполняемому файлу."
    )))
}

fn is_executable(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}

fn is_windows_exe(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = env::temp_dir().join(format!(
                "desklink-exec-{tag}-{}-{unique}",
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

    #[test]
    fn tokenizes_quotes_and_spaces() {
        assert_eq!(
            tokenize("--flag \"two words\" 'single quoted' plain").unwrap(),
            vec!["--flag", "two words", "single quoted", "plain"]
        );
    }

    #[test]
    fn tokenizes_empty_quoted_argument() {
        assert_eq!(tokenize("a \"\" b").unwrap(), vec!["a", "", "b"]);
    }

    #[test]
    fn tokenizes_escaped_space() {
        assert_eq!(tokenize("one\\ two").unwrap(), vec!["one two"]);
    }

    #[test]
    fn rejects_unclosed_quote() {
        let error = tokenize("--flag \"oops").unwrap_err();
        assert!(error.to_string().contains("незакрытая кавычка"));
    }

    #[test]
    fn resolves_relative_path_to_absolute() {
        let temp = TempDir::new("relative");
        let app = temp.path().join("app");
        make_executable(&app);

        let resolved = resolve("./app", temp.path()).unwrap();
        assert_eq!(
            resolved,
            vec![app.canonicalize().unwrap().to_string_lossy()]
        );
    }

    #[test]
    fn resolves_command_through_path() {
        let temp = TempDir::new("path");
        let app = temp.path().join("my-command");
        make_executable(&app);

        let resolved =
            find_in_paths("my-command", temp.path(), vec![temp.path().to_path_buf()]).unwrap();
        assert_eq!(resolved, app.canonicalize().unwrap());
    }

    #[test]
    fn rejects_non_executable_file() {
        let temp = TempDir::new("non-exec");
        let app = temp.path().join("app");
        fs::write(&app, "data").unwrap();

        let error = resolve("./app", temp.path()).unwrap_err();
        assert!(error.to_string().contains("chmod +x"));
    }

    #[test]
    fn rejects_missing_file() {
        let temp = TempDir::new("missing");
        let error = resolve("./nope", temp.path()).unwrap_err();
        assert!(error.to_string().contains("не найден"));
    }

    #[test]
    fn detects_windows_extension_case_insensitively() {
        assert!(is_windows_exe(Path::new("Game.EXE")));
        assert!(!is_windows_exe(Path::new("game.sh")));
    }

    #[test]
    fn resolves_icon_and_directory() {
        let temp = TempDir::new("icon");
        let icon = temp.path().join("icon.png");
        let directory = temp.path().join("work");
        fs::write(&icon, "png").unwrap();
        fs::create_dir(&directory).unwrap();

        assert_eq!(
            resolve_file("./icon.png", temp.path(), "иконка").unwrap(),
            icon.canonicalize().unwrap()
        );
        assert_eq!(
            resolve_directory("./work", temp.path()).unwrap(),
            directory.canonicalize().unwrap()
        );
    }
}
