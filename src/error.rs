use std::fmt;
use std::io;
use std::path::PathBuf;

/// Ошибки приложения. `Usage` отделён от остальных, чтобы вернуть код 2
/// при неверных аргументах командной строки.
#[derive(Debug)]
pub enum Error {
    Usage(String),
    Message(String),
    Io { context: String, source: io::Error },
    AlreadyExists(PathBuf),
    ForeignShortcut(PathBuf),
}

impl Error {
    pub fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }

    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    pub fn io(context: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) => 2,
            _ => 1,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) | Self::Message(message) => write!(f, "{message}"),
            Self::Io { context, source } => write!(f, "{context}: {source}"),
            Self::AlreadyExists(path) => {
                write!(f, "ярлык уже существует: {}", path.display())
            }
            Self::ForeignShortcut(path) => write!(
                f,
                "файл уже существует, но не создан desklink: {}\n\
                 Перезапись запрещена, чтобы не повредить чужой .desktop-файл.",
                path.display()
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
