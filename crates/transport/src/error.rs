use std::fmt;
use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;

#[derive(Debug)]
pub enum TransportError {
    Spawn {
        program: String,
        source: io::Error,
    },
    Failed {
        command: String,
        status: ExitStatus,
        stderr: String,
    },
    /// the command succeeded but printed something this transport can't read.
    Output {
        command: String,
        detail: String,
    },
    Io {
        path: PathBuf,
        source: io::Error,
    },
    /// the implementation on the PATH isn't one this transport speaks to.
    Unsupported(String),
}

impl TransportError {
    pub(crate) fn output(command: &str, detail: impl fmt::Display) -> Self {
        Self::Output {
            command: command.to_owned(),
            detail: detail.to_string(),
        }
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn { program, source } => write!(f, "can't run '{program}': {source}"),
            Self::Failed {
                command,
                status,
                stderr,
            } => write!(f, "'{command}' failed ({status}):\n{}", stderr.trim_end()),
            Self::Output { command, detail } => {
                write!(f, "'{command}' printed something unexpected: {detail}")
            }
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Unsupported(detail) => f.write_str(detail),
        }
    }
}

impl std::error::Error for TransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn { source, .. } | Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
