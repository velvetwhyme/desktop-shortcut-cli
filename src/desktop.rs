//! Генерация и разбор Desktop Entry.

/// Описание ярлыка, готовое к записи в `.desktop`-файл.
#[derive(Debug, Clone)]
pub struct DesktopEntry {
    pub name: String,
    /// Аргументы команды запуска уже без кавычек.
    pub exec: Vec<String>,
    pub icon: Option<String>,
    pub workdir: Option<String>,
    pub terminal: bool,
}

impl DesktopEntry {
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("[Desktop Entry]\n");
        out.push_str("Type=Application\n");
        out.push_str("Version=1.5\n");
        out.push_str(&format!("Name={}\n", escape_value(&self.name)));
        out.push_str(&format!("Exec={}\n", render_exec(&self.exec)));
        out.push_str("Comment=Shortcut created by desklink\n");

        if let Some(icon) = &self.icon {
            out.push_str(&format!("Icon={}\n", escape_value(icon)));
        }
        if let Some(workdir) = &self.workdir {
            out.push_str(&format!("Path={}\n", escape_value(workdir)));
        }

        out.push_str(&format!("Terminal={}\n", self.terminal));
        out.push_str("Categories=Utility;\n");
        out.push_str("X-Label-CLI=true\n");
        out
    }
}

/// Собирает значение `Exec` по правилам Desktop Entry Specification:
/// сначала экранируется командная строка, затем значение ключа.
pub fn render_exec(tokens: &[String]) -> String {
    let quoted = tokens
        .iter()
        .map(|token| quote_exec_arg(token))
        .collect::<Vec<_>>()
        .join(" ");
    escape_value(&quoted)
}

fn quote_exec_arg(arg: &str) -> String {
    // Символ % в Exec — начало field code, поэтому литеральный % удваивается.
    let arg = arg.replace('%', "%%");
    let needs_quotes = arg.is_empty() || arg.chars().any(is_exec_reserved);

    if !needs_quotes {
        return arg;
    }

    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    for ch in arg.chars() {
        if matches!(ch, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

fn is_exec_reserved(ch: char) -> bool {
    matches!(
        ch,
        ' ' | '\t'
            | '\n'
            | '\r'
            | '"'
            | '\''
            | '\\'
            | '>'
            | '<'
            | '~'
            | '|'
            | '&'
            | ';'
            | '$'
            | '*'
            | '?'
            | '#'
            | '('
            | ')'
            | '`'
    )
}

/// Экранирование значения строкового ключа `.desktop`-файла.
pub fn escape_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(ch),
        }
    }
    out
}

/// Обратное преобразование для чтения `Name` из существующего ярлыка.
pub fn unescape_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();

    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }

        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('s') => out.push(' '),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> DesktopEntry {
        DesktopEntry {
            name: "Test App".into(),
            exec: vec!["/opt/Test App/bin".into()],
            icon: Some("/tmp/icon.png".into()),
            workdir: Some("/tmp/work dir".into()),
            terminal: true,
        }
    }

    #[test]
    fn renders_required_keys() {
        let text = entry().render();
        assert!(text.starts_with("[Desktop Entry]\n"));
        assert!(text.contains("Type=Application\n"));
        assert!(text.contains("Name=Test App\n"));
        assert!(text.contains("Exec=\"/opt/Test App/bin\"\n"));
        assert!(text.contains("Icon=/tmp/icon.png\n"));
        assert!(text.contains("Path=/tmp/work dir\n"));
        assert!(text.contains("Terminal=true\n"));
        assert!(text.contains("X-Label-CLI=true\n"));
    }

    #[test]
    fn quotes_exec_arguments_with_spaces_and_specials() {
        let exec = render_exec(&[
            "/path with space/app".into(),
            "plain".into(),
            "say \"hi\"".into(),
            "100%".into(),
            "a\\b".into(),
        ]);

        // В файле Exec проходит два слоя экранирования: quoting аргумента,
        // затем экранирование строкового значения (обратные слэши удваиваются).
        assert_eq!(
            exec,
            "\"/path with space/app\" plain \"say \\\\\"hi\\\\\"\" 100%% \"a\\\\\\\\b\""
        );
    }

    #[test]
    fn escapes_desktop_values() {
        assert_eq!(escape_value("a\\b\nc\td"), "a\\\\b\\nc\\td");
        assert_eq!(unescape_value("a\\\\b\\nc\\td\\s"), "a\\b\nc\td ");
    }

    #[test]
    fn escapes_percent_as_literal() {
        assert_eq!(render_exec(&["100%".into()]), "100%%");
    }

    #[test]
    fn empty_argument_is_quoted() {
        assert_eq!(render_exec(&["".into()]), "\"\"");
    }
}
