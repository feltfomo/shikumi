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
    /// the implementation on the PATH, or the daemon at the other end, isn't one this transport
    /// speaks to, or the operation isn't one it can do there.
    Unsupported(String),
    /// reading from or writing to a daemon connection failed.
    Connection {
        endpoint: String,
        source: io::Error,
    },
    /// the daemon sent something the worker protocol doesn't allow at this point.
    Protocol {
        endpoint: String,
        detail: String,
    },
    /// reading a NAR that was being sent to the daemon failed.
    Nar {
        path: String,
        source: io::Error,
    },
    /// the daemon refused or failed an operation, in its own words, with the last lines it logged
    /// before it did.
    Daemon {
        endpoint: String,
        operation: &'static str,
        message: String,
        log: Vec<String>,
    },
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
            Self::Connection { endpoint, source } => {
                write!(
                    f,
                    "the connection to the daemon at {endpoint} failed: {source}"
                )
            }
            Self::Protocol { endpoint, detail } => {
                write!(
                    f,
                    "the daemon at {endpoint} broke the worker protocol: {detail}"
                )
            }
            Self::Nar { path, source } => write!(f, "reading the NAR of '{path}': {source}"),
            Self::Daemon {
                endpoint,
                operation,
                message,
                log,
            } => {
                write!(
                    f,
                    "the daemon at {endpoint} failed {operation}: {}",
                    message.trim_end()
                )?;
                if !log.is_empty() {
                    write!(f, "\nits last log lines:")?;
                    for line in log {
                        write!(f, "\n  {line}")?;
                    }
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for TransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn { source, .. }
            | Self::Io { source, .. }
            | Self::Connection { source, .. }
            | Self::Nar { source, .. } => Some(source),
            _ => None,
        }
    }
}
