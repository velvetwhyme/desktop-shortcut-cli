//! Разбор аргументов, справка и интерактивные запросы.

use std::io::{self, IsTerminal, Write};
use std::path::Path;

use crate::error::{Error, Result};

pub const HELP: &str = "\
desklink — создание ярлыков .desktop

Использование:
  desklink [ПАРАМЕТРЫ]

Параметры:
  -n, --name <НАЗВАНИЕ>   Отображаемое название ярлыка
  -p, --path <ПУТЬ>       Путь к исполняемому файлу или команда запуска
  -i, --icon <ПУТЬ>       Путь к файлу иконки
  -w, --workdir <ПУТЬ>    Рабочая директория приложения
  -a, --args <АРГУМЕНТЫ>  Аргументы запуска (можно указывать в кавычках)
  -t, --terminal          Запускать приложение в терминале
      --list              Показать ярлыки, созданные desklink
      --remove <НАЗВАНИЕ> Удалить ярлык, созданный desklink
      --force             Перезаписать ярлык desklink без вопроса
  -V, --version           Показать версию
  -h, --help              Показать эту справку

Короткие флаги нельзя объединять: используйте -n \"Имя\" -p /путь,
а не -np. Если не указать обязательные -n и -p, desklink предложит ввести
их в интерактивном режиме.

Примеры:
  desklink -n \"Test App\" -p ./game.exe
  desklink -n \"Test App\" -p ./game.exe -i ./icon.png
  desklink -n \"Редактор\" -p /usr/bin/code -w ~/Projects -t
  desklink --list
  desklink --remove \"Test App\"

Ярлыки сохраняются в ~/.local/share/applications/.
";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub name: Option<String>,
    pub path: Option<String>,
    pub icon: Option<String>,
    pub workdir: Option<String>,
    pub args: Option<String>,
    pub terminal: bool,
    pub list: bool,
    pub remove: Option<String>,
    pub force: bool,
    pub help: bool,
    pub version: bool,
}

pub fn print_help() {
    print!("{HELP}");
}

pub fn parse<I>(arguments: I) -> Result<Args>
where
    I: IntoIterator<Item = String>,
{
    let mut args = Args::default();
    let mut iter = arguments.into_iter().peekable();

    while let Some(argument) = iter.next() {
        if argument == "--" {
            if let Some(extra) = iter.next() {
                return Err(Error::usage(format!(
                    "неожиданный аргумент: «{extra}». Используйте --help."
                )));
            }
            break;
        }

        if argument == "-h" || argument == "--help" {
            args.help = true;
            continue;
        }
        if argument == "-V" || argument == "--version" {
            args.version = true;
            continue;
        }

        if let Some(long) = argument.strip_prefix("--") {
            let (key, inline_value) = match long.split_once('=') {
                Some((key, value)) => (key, Some(value.to_string())),
                None => (long, None),
            };

            match key {
                "name" => args.name = Some(take_value("--name", inline_value, &mut iter)?),
                "path" => args.path = Some(take_value("--path", inline_value, &mut iter)?),
                "icon" => args.icon = Some(take_value("--icon", inline_value, &mut iter)?),
                "workdir" => args.workdir = Some(take_value("--workdir", inline_value, &mut iter)?),
                "args" => args.args = Some(take_value("--args", inline_value, &mut iter)?),
                "remove" => args.remove = Some(take_value("--remove", inline_value, &mut iter)?),
                "terminal" => {
                    reject_inline_value("--terminal", inline_value)?;
                    args.terminal = true;
                }
                "list" => {
                    reject_inline_value("--list", inline_value)?;
                    args.list = true;
                }
                "force" => {
                    reject_inline_value("--force", inline_value)?;
                    args.force = true;
                }
                _ => return Err(unknown_option(&argument)),
            }
            continue;
        }

        if let Some(short) = argument.strip_prefix('-') {
            match short {
                "n" => args.name = Some(take_value("-n", None, &mut iter)?),
                "p" => args.path = Some(take_value("-p", None, &mut iter)?),
                "i" => args.icon = Some(take_value("-i", None, &mut iter)?),
                "w" => args.workdir = Some(take_value("-w", None, &mut iter)?),
                "a" => args.args = Some(take_value("-a", None, &mut iter)?),
                "t" => args.terminal = true,
                _ => {
                    if short.len() > 1
                        && short
                            .chars()
                            .all(|ch| matches!(ch, 'n' | 'p' | 'i' | 'w' | 'a' | 't'))
                    {
                        return Err(Error::usage(format!(
                            "нельзя объединять короткие флаги: «{argument}».\n\
                             Указывайте каждый параметр отдельно, например: -n \"Имя\" -p /путь"
                        )));
                    }
                    return Err(unknown_option(&argument));
                }
            }
            continue;
        }

        return Err(Error::usage(format!(
            "неожиданный аргумент: «{argument}». Используйте --help."
        )));
    }

    if !args.help && !args.version {
        validate_combinations(&args)?;
    }
    Ok(args)
}

fn take_value<I>(
    flag: &str,
    inline_value: Option<String>,
    iter: &mut std::iter::Peekable<I>,
) -> Result<String>
where
    I: Iterator<Item = String>,
{
    if let Some(value) = inline_value {
        return Ok(value);
    }

    iter.next()
        .ok_or_else(|| Error::usage(format!("для параметра {flag} не указано значение")))
}

fn reject_inline_value(flag: &str, inline_value: Option<String>) -> Result<()> {
    if inline_value.is_some() {
        return Err(Error::usage(format!(
            "параметр {flag} не принимает значение"
        )));
    }
    Ok(())
}

fn unknown_option(option: &str) -> Error {
    Error::usage(format!(
        "неизвестный параметр: «{option}».\nИспользуйте --help для списка параметров."
    ))
}

fn validate_combinations(args: &Args) -> Result<()> {
    if args.list && args.remove.is_some() {
        return Err(Error::usage(
            "параметры --list и --remove нельзя использовать одновременно",
        ));
    }

    let has_creation_options = args.name.is_some()
        || args.path.is_some()
        || args.icon.is_some()
        || args.workdir.is_some()
        || args.args.is_some()
        || args.terminal
        || args.force;

    if args.list && has_creation_options {
        return Err(Error::usage(
            "параметр --list нельзя сочетать с параметрами создания ярлыка",
        ));
    }

    if args.remove.is_some() && has_creation_options {
        return Err(Error::usage(
            "параметр --remove нельзя сочетать с параметрами создания ярлыка",
        ));
    }

    Ok(())
}

pub fn stdin_is_terminal() -> bool {
    io::stdin().is_terminal()
}

/// Запрашивает отсутствующие обязательные параметры.
pub fn prompt_missing(args: &mut Args) -> Result<()> {
    if args.name.is_none() {
        args.name = Some(prompt("Название ярлыка: ")?);
    }
    if args.path.is_none() {
        args.path = Some(prompt("Путь к исполняемому файлу или команда: ")?);
    }
    Ok(())
}

pub fn missing_required(args: &Args) -> String {
    let mut missing = Vec::new();
    if args.name.is_none() {
        missing.push("-n, --name");
    }
    if args.path.is_none() {
        missing.push("-p, --path");
    }
    missing.join(", ")
}

pub fn confirm_overwrite(path: &Path) -> Result<bool> {
    let answer = prompt(&format!(
        "Ярлык {} уже существует. Перезаписать? [y/N]: ",
        path.display()
    ))?;
    Ok(matches!(
        answer.trim().to_lowercase().as_str(),
        "y" | "yes" | "д" | "да"
    ))
}

fn prompt(message: &str) -> Result<String> {
    print!("{message}");
    io::stdout()
        .flush()
        .map_err(|error| Error::io("не удалось вывести приглашение", error))?;

    let mut line = String::new();
    let read = io::stdin()
        .read_line(&mut line)
        .map_err(|error| Error::io("не удалось прочитать ввод", error))?;
    if read == 0 {
        return Err(Error::usage("ввод прерван"));
    }

    Ok(line.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(arguments: &[&str]) -> Result<Args> {
        parse(arguments.iter().map(|value| value.to_string()))
    }

    #[test]
    fn parses_all_creation_flags() {
        let args = parse_str(&[
            "-n",
            "Test App",
            "-p",
            "./game.exe",
            "-i",
            "./icon.png",
            "-w",
            ".",
            "-a",
            "--fullscreen",
            "-t",
        ])
        .unwrap();

        assert_eq!(args.name.as_deref(), Some("Test App"));
        assert_eq!(args.path.as_deref(), Some("./game.exe"));
        assert_eq!(args.icon.as_deref(), Some("./icon.png"));
        assert_eq!(args.workdir.as_deref(), Some("."));
        assert_eq!(args.args.as_deref(), Some("--fullscreen"));
        assert!(args.terminal);
    }

    #[test]
    fn parses_long_flags_with_equals() {
        let args =
            parse_str(&["--name=Test App", "--path=./game.exe", "--args=--foo bar"]).unwrap();

        assert_eq!(args.name.as_deref(), Some("Test App"));
        assert_eq!(args.path.as_deref(), Some("./game.exe"));
        assert_eq!(args.args.as_deref(), Some("--foo bar"));
    }

    #[test]
    fn rejects_grouped_short_flags() {
        let error = parse_str(&["-np", "Test App"]).unwrap_err();
        assert_eq!(error.exit_code(), 2);
        assert!(error.to_string().contains("нельзя объединять"));
    }

    #[test]
    fn rejects_unknown_flag() {
        let error = parse_str(&["--nope"]).unwrap_err();
        assert!(error.to_string().contains("неизвестный параметр"));
    }

    #[test]
    fn rejects_missing_value() {
        let error = parse_str(&["-n"]).unwrap_err();
        assert!(error.to_string().contains("не указано значение"));
    }

    #[test]
    fn parses_list_and_remove() {
        assert!(parse_str(&["--list"]).unwrap().list);
        assert_eq!(
            parse_str(&["--remove", "Test App"])
                .unwrap()
                .remove
                .as_deref(),
            Some("Test App")
        );
    }

    #[test]
    fn rejects_conflicting_commands() {
        let error = parse_str(&["--list", "--remove", "Test App"]).unwrap_err();
        assert!(error.to_string().contains("одновременно"));

        let error = parse_str(&["--list", "-n", "Test App"]).unwrap_err();
        assert!(error.to_string().contains("нельзя сочетать"));
    }

    #[test]
    fn reports_missing_required_flags() {
        let args = parse_str(&[]).unwrap();
        assert_eq!(missing_required(&args), "-n, --name, -p, --path");
    }

    #[test]
    fn help_skips_combination_validation() {
        assert!(parse_str(&["--help", "--list"]).unwrap().help);
    }

    #[test]
    fn arguments_starting_with_dash_are_accepted_as_values() {
        let args = parse_str(&["-n", "App", "-p", "sh", "-a", "-c 'echo hi'"]).unwrap();
        assert_eq!(args.args.as_deref(), Some("-c 'echo hi'"));
    }
}
