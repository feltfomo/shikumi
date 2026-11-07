//! one daemon connection, from its handshake to its close, and the pool that bounds how many are
//! open.
//!
//! a connection goes back to the pool only after an operation it carried finished cleanly. any
//! error, and any operation whose future is dropped part way, closes it instead: the stream may
//! hold half a reply, and the daemon drops a connection itself after an error raised before an
//! operation starts (`processConnection` in CppNix 2.35.3 `src/libstore/daemon.cc`, and in Lix
//! 2.95.4 and 2.96.0 `lix/libstore/daemon.cc`). closing also ends whatever the daemon was doing
//! for it: the daemon watches its end for a hangup and interrupts a running build.

use std::collections::VecDeque;
use std::fmt;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use shikumi_derivation::StoreDir;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, Command};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::wire::{
    Decoder, Encoder, MAGIC_ACCESS_DENIED, MAGIC_CLIENT, MAGIC_DAEMON, MINIMUM_MINOR,
    OFFERED_MINOR, RESULT_BUILD_LOG_LINE, STDERR_ERROR, STDERR_LAST, STDERR_NEXT, STDERR_READ,
    STDERR_RESULT, STDERR_START_ACTIVITY, STDERR_STOP_ACTIVITY, STDERR_WRITE, version_word,
};
use crate::{TransportError, Trust};

/// how many of the daemon's last log lines an error carries.
const LOG_LINES: usize = 25;
/// big enough that a NAR goes out in few writes, small enough to cost nothing per connection.
const BUFFER: usize = 64 * 1024;

/// where a daemon is reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// the local daemon's socket, usually `/nix/var/nix/daemon-socket/socket`.
    Socket(PathBuf),
    /// a daemon on another machine, through `ssh <destination> nix-daemon --stdio`, which is what
    /// an `ssh-ng://` store runs on every implementation. the daemon there judges the login user,
    /// so trust follows that user, not this one.
    Ssh(String),
}

impl Endpoint {
    pub fn local() -> Self {
        Self::Socket(PathBuf::from("/nix/var/nix/daemon-socket/socket"))
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Socket(path) => write!(f, "unix://{}", path.display()),
            Self::Ssh(destination) => write!(f, "ssh-ng://{destination}"),
        }
    }
}

/// what the daemon said about itself in the handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hello {
    pub(crate) minor: u8,
    pub(crate) daemon_version: String,
    pub(crate) trust: Trust,
}

pub(crate) type Incoming = Decoder<BufReader<Box<dyn AsyncRead + Send + Unpin>>>;

/// the daemon's side of a connection, with the log lines of the operation in progress.
pub(crate) struct Reader {
    pub(crate) decoder: Incoming,
    log: VecDeque<String>,
}

impl fmt::Debug for Reader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reader")
            .field("endpoint", self.decoder.endpoint())
            .field("log", &self.log)
            .finish_non_exhaustive()
    }
}

impl Reader {
    fn log(&mut self, line: &str) {
        let line = line.trim_end();
        if line.is_empty() {
            return;
        }
        if self.log.len() == LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(line.to_owned());
    }

    /// reads the daemon's log and error stream up to the end of an operation's work, after which
    /// its reply follows. the daemon's own words come back in the error when it fails.
    pub(crate) async fn finish_work(
        &mut self,
        operation: &'static str,
    ) -> Result<(), TransportError> {
        loop {
            match self.decoder.u64().await? {
                STDERR_LAST => {
                    self.log.clear();
                    return Ok(());
                }
                STDERR_ERROR => {
                    let message = self.decoder.error().await?;
                    return Err(TransportError::Daemon {
                        endpoint: self.decoder.endpoint().to_string(),
                        operation,
                        message,
                        log: self.log.drain(..).collect(),
                    });
                }
                STDERR_NEXT => {
                    let line = self.decoder.text().await?;
                    self.log(&line);
                }
                STDERR_START_ACTIVITY => {
                    let _id = self.decoder.u64().await?;
                    let _level = self.decoder.u64().await?;
                    let _kind = self.decoder.u64().await?;
                    let _text = self.decoder.text().await?;
                    self.decoder.fields().await?;
                    let _parent = self.decoder.u64().await?;
                }
                STDERR_STOP_ACTIVITY => {
                    let _id = self.decoder.u64().await?;
                }
                STDERR_RESULT => {
                    let _id = self.decoder.u64().await?;
                    let kind = self.decoder.u64().await?;
                    let field = self.decoder.fields().await?;
                    if kind == RESULT_BUILD_LOG_LINE
                        && let Some(line) = field
                    {
                        self.log(&line);
                    }
                }
                // no daemon sends these to a 1.35 to 1.37 client: they exist for operations this
                // client never sends.
                STDERR_READ | STDERR_WRITE => {
                    return Err(self
                        .decoder
                        .protocol("the daemon asked to stream data an operation doesn't carry"));
                }
                other => {
                    return Err(self
                        .decoder
                        .protocol(format!("unknown log message {other:#x}")));
                }
            }
        }
    }
}

/// this client's side of a connection: requests are encoded, then written in one go.
pub(crate) struct Writer {
    stream: BufWriter<Box<dyn AsyncWrite + Send + Unpin>>,
    endpoint: Arc<str>,
    pub(crate) message: Encoder,
}

impl fmt::Debug for Writer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Writer")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl Writer {
    fn failed(&self, source: std::io::Error) -> TransportError {
        TransportError::Connection {
            endpoint: self.endpoint.to_string(),
            source,
        }
    }

    /// writes the encoded message into the buffer, which goes out when it fills or on `flush`.
    pub(crate) async fn send(&mut self) -> Result<(), TransportError> {
        let bytes = std::mem::take(&mut self.message.bytes);
        let result = self.stream.write_all(&bytes).await;
        self.message.bytes = bytes;
        self.message.bytes.clear();
        result.map_err(|source| self.failed(source))
    }

    pub(crate) async fn flush(&mut self) -> Result<(), TransportError> {
        self.send().await?;
        self.stream
            .flush()
            .await
            .map_err(|source| self.failed(source))
    }

    /// one frame of a framed stream: its length, then its bytes, unpadded.
    pub(crate) async fn frame(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        self.send().await?;
        let written = async {
            self.stream
                .write_all(&(bytes.len() as u64).to_le_bytes())
                .await?;
            self.stream.write_all(bytes).await
        };
        written.await.map_err(|source| self.failed(source))
    }
}

pub(crate) struct Connection {
    pub(crate) reader: Reader,
    pub(crate) writer: Writer,
    /// the ssh process for an `ssh-ng` connection. it's killed if the connection is dropped.
    child: Option<Child>,
    pub(crate) minor: u8,
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("reader", &self.reader)
            .field("minor", &self.minor)
            .field("ssh", &self.child.as_ref().and_then(Child::id))
            .finish_non_exhaustive()
    }
}

impl Connection {
    pub(crate) async fn open(
        endpoint: &Endpoint,
        label: Arc<str>,
    ) -> Result<(Self, Hello), TransportError> {
        let (read, write, child): (
            Box<dyn AsyncRead + Send + Unpin>,
            Box<dyn AsyncWrite + Send + Unpin>,
            _,
        ) = match endpoint {
            Endpoint::Socket(path) => {
                let stream = tokio::net::UnixStream::connect(path)
                    .await
                    .map_err(|source| TransportError::Connection {
                        endpoint: label.to_string(),
                        source,
                    })?;
                let (read, write) = stream.into_split();
                (Box::new(read), Box::new(write), None)
            }
            Endpoint::Ssh(destination) => {
                // no agent or X11 forwarding and no tty, and no prompt a program can't answer;
                // the remote program is what Nix's own `ssh-ng` client runs. ssh's own messages
                // go to this process's stderr, as Nix's do.
                let mut child = Command::new("ssh")
                    .args([
                        destination.as_str(),
                        "-x",
                        "-a",
                        "-T",
                        "-o",
                        "BatchMode=yes",
                    ])
                    .args(["--", "nix-daemon", "--stdio"])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit())
                    .kill_on_drop(true)
                    .spawn()
                    .map_err(|source| TransportError::Spawn {
                        program: "ssh".to_owned(),
                        source,
                    })?;
                let read = child.stdout.take().expect("stdout is piped");
                let write = child.stdin.take().expect("stdin is piped");
                (Box::new(read), Box::new(write), Some(child))
            }
        };
        let mut connection = Self {
            reader: Reader {
                decoder: Decoder::new(BufReader::with_capacity(BUFFER, read), label.clone()),
                log: VecDeque::new(),
            },
            writer: Writer {
                stream: BufWriter::with_capacity(BUFFER, write),
                endpoint: label,
                message: Encoder::default(),
            },
            child,
            minor: 0,
        };
        let hello = connection.handshake().await?;
        connection.minor = hello.minor;
        Ok((connection, hello))
    }

    /// the version exchange and the daemon's account of itself, as `BasicClientConnection`'s
    /// `handshake` and `postHandshake` do it in CppNix 2.35.3
    /// `src/libstore/worker-protocol-connection.cc`.
    async fn handshake(&mut self) -> Result<Hello, TransportError> {
        self.writer
            .message
            .u64(MAGIC_CLIENT)
            .u64(version_word(OFFERED_MINOR));
        self.writer.flush().await?;
        let decoder = &mut self.reader.decoder;
        match decoder.u64().await? {
            MAGIC_DAEMON => {}
            MAGIC_ACCESS_DENIED => {
                return Err(TransportError::Unsupported(format!(
                    "the daemon at {} denied access; check its 'allowed-users' setting",
                    decoder.endpoint()
                )));
            }
            other => {
                return Err(decoder.protocol(format!("greeting {other:#x} isn't the daemon's")));
            }
        }
        let word = decoder.u64().await?;
        let (major, minor) = (word >> 8, (word & 0xff) as u8);
        if major != 1 || minor < MINIMUM_MINOR {
            return Err(TransportError::Unsupported(format!(
                "the daemon at {} speaks worker protocol {major}.{minor}; this client needs 1.{MINIMUM_MINOR} or later",
                decoder.endpoint()
            )));
        }
        // Lix answers with its own 1.35 whatever is offered and never lowers it, and CppNix lowers
        // to what's offered, so the smaller of the two is what both ends speak.
        let minor = minor.min(OFFERED_MINOR);
        // the obsolete CPU affinity flag and reserve-space flag, both off.
        self.writer.message.u64(0).u64(0);
        self.writer.flush().await?;
        let decoder = &mut self.reader.decoder;
        let daemon_version = decoder.text().await?;
        let trust = match decoder.u64().await? {
            0 => Trust::Unknown,
            1 => Trust::Trusted,
            2 => Trust::Untrusted,
            other => return Err(decoder.protocol(format!("trust word {other}"))),
        };
        self.reader.finish_work("open the connection").await?;
        Ok(Hello {
            minor,
            daemon_version,
            trust,
        })
    }

    /// ends the connection the way the daemon expects a client to: no more requests, then wait
    /// until the daemon has hung up its end, so whatever it held for this connection is released
    /// by the time this returns.
    async fn close(mut self) -> Result<(), TransportError> {
        let endpoint = self.writer.endpoint.to_string();
        let failed = |source| TransportError::Connection {
            endpoint: endpoint.clone(),
            source,
        };
        self.writer.flush().await?;
        self.writer.stream.shutdown().await.map_err(failed)?;
        drop(self.writer);
        tokio::io::copy(self.reader.decoder.reader(), &mut tokio::io::sink())
            .await
            .map_err(failed)?;
        if let Some(mut child) = self.child.take() {
            child.wait().await.map_err(failed)?;
        }
        Ok(())
    }
}

/// at most `limit` connections, opened when first needed and kept for the next operation.
#[derive(Debug)]
pub(crate) struct Pool {
    endpoint: Endpoint,
    label: Arc<str>,
    pub(crate) dir: StoreDir,
    idle: Mutex<Vec<Connection>>,
    slots: Arc<Semaphore>,
    limit: usize,
}

impl Pool {
    pub(crate) fn new(endpoint: Endpoint, dir: StoreDir, limit: NonZeroUsize) -> Self {
        Self {
            label: Arc::from(endpoint.to_string()),
            endpoint,
            dir,
            idle: Mutex::new(Vec::new()),
            slots: Arc::new(Semaphore::new(limit.get())),
            limit: limit.get(),
        }
    }

    pub(crate) fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub(crate) fn limit(&self) -> usize {
        self.limit
    }

    /// waits for a free slot and returns an idle connection, or a new one.
    pub(crate) async fn acquire(self: &Arc<Self>) -> Result<Pooled, TransportError> {
        let permit = self
            .slots
            .clone()
            .acquire_owned()
            .await
            .expect("the pool's semaphore is never closed");
        let idle = self
            .idle
            .lock()
            .expect("no panic holds the pool's lock")
            .pop();
        let connection = match idle {
            Some(connection) => connection,
            None => {
                Connection::open(&self.endpoint, self.label.clone())
                    .await?
                    .0
            }
        };
        Ok(Pooled {
            connection: Some(connection),
            pool: self.clone(),
            _permit: permit,
        })
    }

    /// opens a connection outside any slot, for the handshake that tells a new store who it's
    /// talking to, and keeps it idle for the first operation.
    pub(crate) async fn greet(&self) -> Result<Hello, TransportError> {
        let (connection, hello) = Connection::open(&self.endpoint, self.label.clone()).await?;
        self.idle
            .lock()
            .expect("no panic holds the pool's lock")
            .push(connection);
        Ok(hello)
    }

    /// closes every idle connection and waits for each daemon to hang up.
    pub(crate) async fn close_idle(&self) -> Result<(), TransportError> {
        let idle = std::mem::take(&mut *self.idle.lock().expect("no panic holds the pool's lock"));
        for connection in idle {
            connection.close().await?;
        }
        Ok(())
    }
}

/// a connection out of the pool. dropped, it closes the connection; only [`Pooled::release`]
/// returns it for reuse, which a caller does once an operation's reply has been read in full.
#[derive(Debug)]
pub(crate) struct Pooled {
    connection: Option<Connection>,
    pool: Arc<Pool>,
    _permit: OwnedSemaphorePermit,
}

impl Pooled {
    pub(crate) fn release(mut self) {
        let connection = self
            .connection
            .take()
            .expect("a pooled connection is released once");
        self.pool
            .idle
            .lock()
            .expect("no panic holds the pool's lock")
            .push(connection);
    }
}

impl std::ops::Deref for Pooled {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        self.connection.as_ref().expect("held until released")
    }
}

impl std::ops::DerefMut for Pooled {
    fn deref_mut(&mut self) -> &mut Connection {
        self.connection.as_mut().expect("held until released")
    }
}
