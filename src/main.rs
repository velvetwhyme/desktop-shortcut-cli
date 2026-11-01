mod cli;
mod desktop;
mod error;
mod exec;
mod shortcut;

use std::env;
use std::process::ExitCode;

use error::{Error, Result};
use shortcut::ShortcutSpec;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("desklink: {error}");
            ExitCode::from(error.exit_code() as u8)
        }
    }
}

fn run() -> Result<()> {
    let raw_arguments = env::args().skip(1).collect::<Vec<_>>();

    // Без аргументов в неинтерактивном окружении показываем справку.
    if raw_arguments.is_empty() && !cli::stdin_is_terminal() {
        cli::print_help();
        return Ok(());
    }

    let mut args = cli::parse(raw_arguments)?;

    if args.help {
        cli::print_help();
        return Ok(());
    }
    if args.version {
        println!("desklink {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let cwd = env::current_dir()
        .map_err(|error| Error::io("не удалось определить текущий каталог", error))?;
    let apps_dir = shortcut::applications_dir()?;

    if args.list {
        let shortcuts = shortcut::list(&apps_dir)?;
        if shortcuts.is_empty() {
            println!("Ярлыков, созданных desklink, не найдено.");
            return Ok(());
        }

        println!("Ярлыки, созданные desklink ({}):", shortcuts.len());
        for shortcut in shortcuts {
            println!("  {}", shortcut.name);
            println!("    файл:   {}", shortcut.path.display());
            if !shortcut.exec.is_empty() {
                println!("    запуск: {}", shortcut.exec);
            }
        }
        return Ok(());
    }

    if let Some(query) = args.remove.take() {
        let removed = shortcut::remove(&apps_dir, &query)?;
        for path in removed {
            println!("Удалён: {}", path.display());
        }
        return Ok(());
    }

    if args.name.is_none() || args.path.is_none() {
        if cli::stdin_is_terminal() {
            cli::prompt_missing(&mut args)?;
        } else {
            return Err(Error::usage(format!(
                "не хватает обязательных параметров: {}.\n\
                 Запустите команду в терминале для интерактивного ввода или используйте --help.",
                cli::missing_required(&args)
            )));
        }
    }

    let spec = ShortcutSpec {
        name: args.name.unwrap_or_default(),
        path: args.path.unwrap_or_default(),
        icon: args.icon,
        workdir: args.workdir,
        args: args.args,
        terminal: args.terminal,
    };

    let prepared = shortcut::prepare(&spec, &cwd)?;

    let target = match shortcut::create(&prepared, &apps_dir, args.force) {
        Ok(path) => path,
        Err(Error::AlreadyExists(path)) if !args.force => {
            if cli::stdin_is_terminal() {
                if cli::confirm_overwrite(&path)? {
                    shortcut::create(&prepared, &apps_dir, true)?
                } else {
                    println!("Перезапись отменена.");
                    return Ok(());
                }
            } else {
                return Err(Error::message(format!(
                    "ярлык уже существует: {}\n\
                     Используйте --force, чтобы перезаписать его.",
                    path.display()
                )));
            }
        }
        Err(error) => return Err(error),
    };

    println!("Ярлык создан: {}", target.display());
    Ok(())
}
