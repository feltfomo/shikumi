//! the store transport over the daemon's worker protocol, the one programmatic interface CppNix,
//! Determinate Nix and Lix share, over a unix socket or `ssh-ng`.
//!
//! - validity and builds are one request for a whole batch. path info has no batched request, so
//!   a batch's requests are written back to back on one connection while its replies are read,
//!   and large batches are split across connections; no path waits for another's round trip.
//! - `.drv` files and sources go to the daemon in one `AddMultipleToStore` stream, ordered so each
//!   path's references arrive before it. each is content-addressed, so the daemon re-derives every
//!   path from what it's sent and refuses one that doesn't reproduce.
//! - a NAR comes back from `NarFromPath` as it's read, on a connection of its own.
//!
//! connections come from two pools of the same size: one for operations, one for NAR reads. an
//! add whose sources are NARs read from this store holds one of each, and with one pool a batch of
//! such adds could hold every slot while each waits for another.

mod connection;
#[cfg(feature = "trust-probes")]
mod probes;
mod wire;

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::future::Future;
use std::io;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, ready};

use shikumi_derivation::{NarNode, OutputName, StoreDerivation, StoreDir, StorePath};
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};

use self::connection::{Connection, Hello, Pool, Pooled, Writer};
use self::wire::{BuildResult, Encoder, op};
use crate::jobs::{Graph, join_all};
use crate::nar::{Lazy, Verified};
use crate::{
    BuildTarget, GcRoot, NarReader, PathInfo, SourceNar, StoreTransport, TransportError, Trust,
};

pub use self::connection::Endpoint;

/// the fewest paths worth a connection of their own. below this a split costs more in handshakes
/// and daemon workers than the parallel replies save.
const CHUNK: usize = 256;
/// how much of a NAR goes into one frame.
const FRAME: usize = 64 * 1024;

#[derive(Debug)]
pub struct WorkerStore {
    operations: Arc<Pool>,
    nars: Arc<Pool>,
    hello: Hello,
}

impl WorkerStore {
    /// opens one connection to learn the daemon's version and trust, and keeps it for the first
    /// operation. at most `connections` operations and `connections` NAR reads run at once; more
    /// wait for a slot. `store_dir` must be the daemon's store, since every path is computed
    /// against it.
    pub async fn connect(
        endpoint: Endpoint,
        store_dir: StoreDir,
        connections: NonZeroUsize,
    ) -> Result<Self, TransportError> {
        let operations = Arc::new(Pool::new(endpoint.clone(), store_dir.clone(), connections));
        let hello = operations.greet().await?;
        let nars = Arc::new(Pool::new(endpoint, store_dir, connections));
        Ok(Self {
            operations,
            nars,
            hello,
        })
    }

    pub fn endpoint(&self) -> &Endpoint {
        self.operations.endpoint()
    }

    /// the daemon's version as its handshake gives it, such as `2.35.3` or `2.95.4`.
    pub fn daemon_version(&self) -> &str {
        &self.hello.daemon_version
    }

    /// the protocol version both ends speak, such as `1.37`.
    pub fn protocol(&self) -> String {
        format!("1.{}", self.hello.minor)
    }

    fn dir(&self) -> &StoreDir {
        &self.operations.dir
    }

    /// closes every connection and waits for each daemon worker to hang up, so whatever it held
    /// for this client, such as temporary roots on the paths it added, is released by the time
    /// this returns. a NAR reader still being read keeps its own connection until it's dropped.
    pub async fn close(self) -> Result<(), TransportError> {
        self.operations.close_idle().await?;
        self.nars.close_idle().await
    }

    /// sends one `AddMultipleToStore`: a count, then each path's info and exactly its NAR's bytes,
    /// all inside one framed stream, in an order where references come first.
    ///
    /// a source's NAR opens only when it's first read, which for a NAR read from a daemon is a
    /// round trip. so while one NAR streams, the next ones are opened and their first bytes read,
    /// and a batch of small changed outputs doesn't pay a round trip each in series. the stream
    /// never waits for that: whatever is still opening when the current NAR ends carries on when
    /// its turn comes, since a reader keeps its own progress.
    async fn add_paths(&self, items: Vec<Added<'_>>) -> Result<(), TransportError> {
        if items.is_empty() {
            return Ok(());
        }
        let order = reference_order(&items)?;
        let mut items: Vec<Option<Added<'_>>> = items.into_iter().map(Some).collect();
        let mut queue: VecDeque<Queued<'_>> = order
            .into_iter()
            .map(|index| Queued {
                added: items[index].take().expect("each item is sent once"),
                head: None,
            })
            .collect();
        let dir = self.dir();
        let mut connection = self.operations.acquire().await?;
        connection
            .writer
            .message
            .u64(op::ADD_MULTIPLE_TO_STORE)
            // no repair, and signatures checked: an untrusted client gets both anyway.
            .u64(0)
            .u64(0);
        connection.writer.send().await?;
        let Connection { reader, writer, .. } = &mut *connection;
        let write = async {
            let mut header = Encoder::default();
            header.u64(queue.len() as u64);
            writer.frame(&header.bytes).await?;
            let mut buffer = vec![0; FRAME];
            while let Some(current) = queue.pop_front() {
                let send = send(writer, dir, &mut header, &mut buffer, current);
                let ahead = prime(queue.iter_mut().take(AHEAD));
                tokio::pin!(send, ahead);
                let mut primed = false;
                loop {
                    tokio::select! {
                        sent = &mut send => {
                            sent?;
                            break;
                        }
                        opened = &mut ahead, if !primed => {
                            opened?;
                            primed = true;
                        }
                    }
                }
            }
            writer.frame(&[]).await?;
            writer.flush().await
        };
        exchange(write, reader.finish_work("add paths")).await?;
        connection.release();
        Ok(())
    }

    /// splits a batch of paths into at most one chunk per connection, each worth a connection, and
    /// runs `operation` over every chunk at once, in this task. the chunks borrow the batch, so
    /// no path is copied to cross a task boundary.
    async fn fan_out<'a, T, F, O>(
        &self,
        paths: &'a [StorePath],
        operation: O,
    ) -> Result<Vec<T>, TransportError>
    where
        F: Future<Output = Result<T, TransportError>>,
        O: Fn(&'a [StorePath]) -> F,
    {
        let size = paths.len().div_ceil(self.operations.limit()).max(CHUNK);
        join_all(paths.chunks(size).map(operation).collect()).await
    }

    fn failed(&self, operation: &'static str, lines: Vec<String>) -> TransportError {
        TransportError::Daemon {
            endpoint: self.endpoint().to_string(),
            operation,
            message: lines.join("\n"),
            log: Vec::new(),
        }
    }
}

impl StoreTransport for WorkerStore {
    async fn trust(&self) -> Result<Trust, TransportError> {
        Ok(self.hello.trust)
    }

    async fn add_derivations(
        &self,
        derivations: &[&StoreDerivation],
    ) -> Result<Vec<StorePath>, TransportError> {
        // a `.drv` is a text file addressed by its contents and references; the daemon checks the
        // address, and on registering it checks the output paths inside against its own
        // computation (`registerValidPath` in CppNix 2.35.3 `src/libstore/local-store.cc`).
        self.add_paths(
            derivations
                .iter()
                .map(|drv| Added::Derivation(drv))
                .collect(),
        )
        .await?;
        Ok(derivations.iter().map(|drv| drv.path().clone()).collect())
    }

    async fn add_sources(&self, sources: Vec<SourceNar>) -> Result<(), TransportError> {
        self.add_paths(sources.into_iter().map(Added::Source).collect())
            .await
    }

    async fn build(
        &self,
        targets: &[BuildTarget],
    ) -> Result<Vec<BTreeMap<OutputName, StorePath>>, TransportError> {
        if targets.is_empty() {
            return Ok(Vec::new());
        }
        let dir = self.dir();
        // `<drv>!<output>,<output>`, the form `DerivedPath` takes on the wire from 1.30.
        let keys: Vec<String> = targets
            .iter()
            .map(|target| {
                let outputs: Vec<&str> = target.outputs.iter().map(OutputName::as_str).collect();
                format!("{}!{}", dir.display(&target.derivation), outputs.join(","))
            })
            .collect();
        let mut connection = self.operations.acquire().await?;
        connection
            .writer
            .message
            .u64(op::BUILD_PATHS_WITH_RESULTS)
            .strings(keys.iter().map(String::as_str))
            .u64(0);
        connection.writer.flush().await?;
        connection.reader.finish_work("build").await?;
        let minor = connection.minor;
        let decoder = &mut connection.reader.decoder;
        let mut results: HashMap<String, BuildResult> = HashMap::with_capacity(keys.len());
        for _ in 0..decoder.count().await? {
            let key = decoder.string().await?;
            results.insert(key, decoder.build_result(dir, minor).await?);
        }
        connection.release();
        let mut failures = Vec::new();
        let mut built = Vec::with_capacity(targets.len());
        for (target, key) in targets.iter().zip(&keys) {
            let Some(result) = results.get(key) else {
                failures.push(format!("{key}: no result"));
                built.push(BTreeMap::new());
                continue;
            };
            if !result.status.succeeded() {
                failures.push(result_line(key, result));
            }
            let mut outputs = BTreeMap::new();
            for name in &target.outputs {
                match result.outputs.get(name.as_str()) {
                    Some(path) => {
                        outputs.insert(name.clone(), path.clone());
                    }
                    None if result.status.succeeded() => {
                        failures.push(format!("{key}: no path reported for output '{name}'"));
                    }
                    None => {}
                }
            }
            built.push(outputs);
        }
        if failures.is_empty() {
            Ok(built)
        } else {
            Err(self.failed("build", failures))
        }
    }

    async fn query_valid(
        &self,
        paths: &[StorePath],
    ) -> Result<BTreeSet<StorePath>, TransportError> {
        let dir = self.dir();
        let valid = self
            .fan_out(paths, |chunk| async move {
                let mut connection = self.operations.acquire().await?;
                connection
                    .writer
                    .message
                    .u64(op::QUERY_VALID_PATHS)
                    .paths(dir, chunk.iter())
                    // don't substitute: validity here means in this store now.
                    .u64(0);
                connection.writer.flush().await?;
                connection.reader.finish_work("query validity").await?;
                let valid = connection.reader.decoder.paths(dir).await?;
                connection.release();
                Ok(valid)
            })
            .await?;
        Ok(valid.into_iter().flatten().collect())
    }

    async fn query_path_info(&self, paths: &[StorePath]) -> Result<Vec<PathInfo>, TransportError> {
        let dir = self.dir();
        let infos = self
            .fan_out(paths, |chunk| async move {
                let mut connection = self.operations.acquire().await?;
                let Connection { reader, writer, .. } = &mut *connection;
                let write = async {
                    for path in chunk {
                        writer.message.u64(op::QUERY_PATH_INFO).path(dir, path);
                        writer.send().await?;
                    }
                    writer.flush().await
                };
                let read = async {
                    let mut infos = Vec::with_capacity(chunk.len());
                    for path in chunk {
                        reader.finish_work("query path info").await?;
                        if !reader.decoder.bool().await? {
                            return Err(self.failed(
                                "query path info",
                                vec![format!("'{}' isn't valid", dir.display(path))],
                            ));
                        }
                        infos.push(reader.decoder.path_info(dir, path.clone()).await?);
                    }
                    Ok(infos)
                };
                let infos = exchange(write, read).await?;
                connection.release();
                Ok(infos)
            })
            .await?;
        Ok(infos.into_iter().flatten().collect())
    }

    fn read_nar(&self, info: &PathInfo) -> NarReader {
        let pool = self.nars.clone();
        let path = info.path.clone();
        let size = info.nar_size;
        let opening = async move {
            let mut connection = pool.acquire().await.map_err(io::Error::other)?;
            connection
                .writer
                .message
                .u64(op::NAR_FROM_PATH)
                .path(&pool.dir, &path);
            connection.writer.flush().await.map_err(io::Error::other)?;
            connection
                .reader
                .finish_work("read a NAR")
                .await
                .map_err(io::Error::other)?;
            // the NAR follows unframed, and nothing marks its end but its size.
            Ok(Box::new(ConnectionNar {
                connection: Some(connection),
                left: size,
            }) as NarReader)
        };
        Box::new(Verified::new(
            Lazy::new(Box::pin(opening)),
            self.dir().display(&info.path),
            info,
        ))
    }

    async fn add_roots(&self, roots: &[GcRoot]) -> Result<(), TransportError> {
        if roots.is_empty() {
            return Ok(());
        }
        if let Endpoint::Ssh(destination) = self.endpoint() {
            return Err(TransportError::Unsupported(format!(
                "an indirect root is a link on the daemon's machine, and the daemon at ssh-ng://{destination} is reached through ssh; root paths there from that machine"
            )));
        }
        let links = make_links(
            roots
                .iter()
                .map(|root| (root.link.clone(), self.dir().display(&root.path)))
                .collect(),
        )
        .await?;
        let mut connection = self.operations.acquire().await?;
        let Connection { reader, writer, .. } = &mut *connection;
        let write = async {
            for link in &links {
                writer
                    .message
                    .u64(op::ADD_INDIRECT_ROOT)
                    .string(&link.to_string_lossy());
                writer.send().await?;
            }
            writer.flush().await
        };
        let read = async {
            for _ in &links {
                reader.finish_work("add an indirect root").await?;
                reader.decoder.u64().await?;
            }
            Ok(())
        };
        exchange(write, read).await?;
        connection.release();
        Ok(())
    }
}

/// writes a request while reading its replies, so neither side blocks on a full pipe: the daemon
/// logs while it reads, and pipelined replies arrive while requests still go out. the first error
/// from either side ends both.
async fn exchange<T>(
    write: impl Future<Output = Result<(), TransportError>>,
    read: impl Future<Output = Result<T, TransportError>>,
) -> Result<T, TransportError> {
    tokio::pin!(write, read);
    let mut written = false;
    loop {
        tokio::select! {
            result = &mut write, if !written => {
                result?;
                written = true;
            }
            result = &mut read => {
                let value = result?;
                if !written {
                    write.await?;
                }
                return Ok(value);
            }
        }
    }
}

/// the order to send paths in so each one's references in the batch arrive first, keeping input
/// order otherwise.
fn reference_order(items: &[Added<'_>]) -> Result<Vec<usize>, TransportError> {
    let index: HashMap<&StorePath, usize> = items
        .iter()
        .enumerate()
        .map(|(i, item)| (item.path(), i))
        .collect();
    let waits_for: Vec<Vec<usize>> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            item.references()
                .filter_map(|reference| index.get(reference).copied())
                .filter(|&j| j != i)
                .collect()
        })
        .collect();
    Graph::new(&waits_for).order()
}

/// one path an `AddMultipleToStore` stream carries. a source's NAR comes from its reader; a
/// `.drv`'s is rendered from its text only when the stream reaches it, so a batch of them holds one
/// NAR at a time.
enum Added<'a> {
    Source(SourceNar),
    Derivation(&'a StoreDerivation),
}

impl Added<'_> {
    fn path(&self) -> &StorePath {
        match self {
            Self::Source(source) => source.address.path(),
            Self::Derivation(drv) => drv.path(),
        }
    }

    /// what the path refers to, which in a batch has to arrive before it.
    fn references(&self) -> Box<dyn Iterator<Item = &StorePath> + '_> {
        match self {
            Self::Source(source) => Box::new(source.address.references().iter()),
            Self::Derivation(drv) => Box::new(
                drv.input_sources()
                    .iter()
                    .chain(drv.input_derivations().map(|(path, _)| path)),
            ),
        }
    }
}

/// a path waiting its turn in the stream, with the first bytes of its NAR once they're read.
struct Queued<'a> {
    added: Added<'a>,
    head: Option<Vec<u8>>,
}

/// how many sources past the one streaming have their NARs opened early.
const AHEAD: usize = 2;

/// opens the NARs of the sources about to be sent by reading their first bytes, all at once.
async fn prime<'q, 'a: 'q>(
    ahead: impl Iterator<Item = &'q mut Queued<'a>>,
) -> Result<(), TransportError> {
    let opening = ahead
        .filter(|queued| queued.head.is_none())
        .filter_map(|queued| match &mut queued.added {
            Added::Source(source) => Some((source, &mut queued.head)),
            Added::Derivation(_) => None,
        })
        .map(|(source, head)| async move {
            let want = usize::try_from(source.nar_size.min(FRAME as u64)).expect("at most FRAME");
            let mut first = vec![0; want];
            let read = source
                .nar
                .read(&mut first)
                .await
                .map_err(|error| TransportError::Nar {
                    path: source.address.path().to_string(),
                    source: error,
                })?;
            first.truncate(read);
            *head = Some(first);
            Ok(())
        })
        .collect();
    join_all(opening).await.map(drop)
}

/// one path's info and exactly its NAR's bytes, as frames.
async fn send(
    writer: &mut Writer,
    dir: &StoreDir,
    header: &mut Encoder,
    buffer: &mut [u8],
    queued: Queued<'_>,
) -> Result<(), TransportError> {
    header.bytes.clear();
    match queued.added {
        Added::Derivation(drv) => {
            let nar = NarNode::regular(false, drv.aterm().as_bytes().to_vec())
                .to_nar()
                .expect("a regular file always serializes");
            header.path_info(
                dir,
                drv.path(),
                drv.references().iter(),
                (&nar.hash, nar.bytes.len() as u64),
                drv.content_address(),
            );
            writer.frame(&header.bytes).await?;
            for chunk in nar.bytes.chunks(buffer.len()) {
                writer.frame(chunk).await?;
            }
            Ok(())
        }
        Added::Source(mut source) => {
            let address = &source.address;
            header.path_info(
                dir,
                address.path(),
                address.references().iter(),
                (address.nar_hash(), source.nar_size),
                address.content_address(),
            );
            writer.frame(&header.bytes).await?;
            let path = dir.display(address.path());
            let failed = |error| TransportError::Nar {
                path: path.clone(),
                source: error,
            };
            let mut left = source.nar_size;
            if let Some(head) = queued.head {
                writer.frame(&head).await?;
                left -= head.len() as u64;
                if head.is_empty() && left > 0 {
                    return Err(failed(short(left, source.nar_size)));
                }
            }
            while left > 0 {
                let want = buffer
                    .len()
                    .min(usize::try_from(left).unwrap_or(usize::MAX));
                let got = source.nar.read(&mut buffer[..want]).await.map_err(failed)?;
                if got == 0 {
                    return Err(failed(short(left, source.nar_size)));
                }
                writer.frame(&buffer[..got]).await?;
                left -= got as u64;
            }
            // the daemon reads exactly the declared size, so bytes past it would be lost
            // silently, and a reader that verifies its NAR only fails once it's read to its end.
            if source.nar.read(&mut buffer[..1]).await.map_err(failed)? != 0 {
                return Err(failed(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("runs past its declared {} bytes", source.nar_size),
                )));
            }
            Ok(())
        }
    }
}

fn short(left: u64, size: u64) -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        format!("ended {left} bytes short of its declared {size}"),
    )
}

fn result_line(key: &str, result: &BuildResult) -> String {
    let mut line = format!("{key}: {}", result.status);
    if !result.error.is_empty() {
        line.push_str(": ");
        line.push_str(result.error.trim_end());
    }
    line
}

fn absolute(path: &Path) -> Result<PathBuf, TransportError> {
    std::path::absolute(path).map_err(|source| TransportError::Io {
        path: path.to_owned(),
        source,
    })
}

/// points each link at its target, replacing an older link but never a file or directory, which
/// would be someone's data. every link appears in one rename, so a collector running meanwhile sees
/// the old root or the new one, never neither. the whole batch is one blocking task, since each
/// link takes several filesystem calls.
async fn make_links(links: Vec<(PathBuf, String)>) -> Result<Vec<PathBuf>, TransportError> {
    tokio::task::spawn_blocking(move || {
        links
            .into_iter()
            .map(|(link, target)| make_link(&link, &target))
            .collect()
    })
    .await
    .expect("making links doesn't panic")
}

fn make_link(link: &Path, target: &str) -> Result<PathBuf, TransportError> {
    let link = absolute(link)?;
    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| TransportError::Io { path, source }
    };
    if let Ok(existing) = std::fs::symlink_metadata(&link)
        && !existing.file_type().is_symlink()
    {
        return Err(TransportError::Io {
            path: link,
            source: io::Error::new(io::ErrorKind::AlreadyExists, "exists and isn't a symlink"),
        });
    }
    let parent = link.parent().unwrap_or(Path::new("/"));
    std::fs::create_dir_all(parent).map_err(io(parent))?;
    let temporary = temporary(&link);
    std::os::unix::fs::symlink(target, &temporary).map_err(io(&temporary))?;
    std::fs::rename(&temporary, &link).map_err(io(&link))?;
    Ok(link)
}

/// a name beside `link` that no other call uses, in this process or another: the process id
/// separates processes, and a counter separates the calls of one.
fn temporary(link: &Path) -> PathBuf {
    static CALLS: AtomicU64 = AtomicU64::new(0);
    let mut temporary = link.to_owned().into_os_string();
    temporary.push(format!(
        ".tmp-{}-{}",
        std::process::id(),
        CALLS.fetch_add(1, Ordering::Relaxed)
    ));
    PathBuf::from(temporary)
}

/// the NAR `NarFromPath` sends after its reply, read straight off the connection. the connection
/// goes back to the pool once the last byte is read; dropped before then, it's closed, since the
/// rest of the NAR would still be on it.
struct ConnectionNar {
    connection: Option<Pooled>,
    left: u64,
}

impl AsyncRead for ConnectionNar {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let Some(connection) = this.connection.as_mut() else {
            return Poll::Ready(Ok(()));
        };
        if this.left == 0 {
            if let Some(connection) = this.connection.take() {
                connection.release();
            }
            return Poll::Ready(Ok(()));
        }
        let source = Pin::new(connection.reader.decoder.reader());
        let read = if buf.remaining() as u64 <= this.left {
            let before = buf.filled().len();
            ready!(source.poll_read(cx, buf))?;
            buf.filled().len() - before
        } else {
            let want = usize::try_from(this.left).expect("less than a buffer's remaining space");
            let mut limited = ReadBuf::new(buf.initialize_unfilled_to(want));
            ready!(source.poll_read(cx, &mut limited))?;
            let read = limited.filled().len();
            buf.advance(read);
            read
        };
        if read == 0 && buf.remaining() > 0 {
            this.connection = None;
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!(
                    "the daemon closed the connection {} bytes before the NAR's end",
                    this.left
                ),
            )));
        }
        this.left -= read as u64;
        if this.left == 0
            && let Some(connection) = this.connection.take()
        {
            connection.release();
        }
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{UnixListener, UnixStream};

    use super::wire::{MAGIC_DAEMON, STDERR_ERROR, STDERR_LAST, STDERR_NEXT, version_word};

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    /// the daemon's half of a 1.35 handshake, as Lix 2.95.4 answers it, for an untrusted client.
    async fn greet(stream: &mut UnixStream) {
        let mut hello = [0; 16];
        stream.read_exact(&mut hello).await.unwrap();
        let mut reply = Encoder::default();
        reply.u64(MAGIC_DAEMON).u64(version_word(35));
        stream.write_all(&reply.bytes).await.unwrap();
        let mut flags = [0; 16];
        stream.read_exact(&mut flags).await.unwrap();
        let mut reply = Encoder::default();
        reply.string("2.95.4").u64(2).u64(STDERR_LAST);
        stream.write_all(&reply.bytes).await.unwrap();
    }

    async fn read_op(stream: &mut UnixStream) -> u64 {
        let mut word = [0; 8];
        stream.read_exact(&mut word).await.unwrap();
        u64::from_le_bytes(word)
    }

    fn target() -> BuildTarget {
        BuildTarget {
            derivation: StorePath::from_base_name("0mdqa9w1p6cmli6976v4wi0sw9r4p5pr-a.drv")
                .unwrap(),
            outputs: BTreeSet::from([OutputName::new("out").unwrap()]),
        }
    }

    // two roots of one link made at once, as two commands writing the same root would, each need
    // a temporary of their own: with a shared one, one call's cleanup or rename takes the other's
    // link, and that call fails or roots the wrong path.
    #[test]
    fn two_links_made_at_once_never_share_a_temporary() {
        let link = Path::new("/work/roots/current");
        assert_ne!(temporary(link), temporary(link));
    }

    #[test]
    fn a_link_replaces_an_older_link_but_never_a_file() {
        let directory = tempfile::tempdir().unwrap();
        let link = directory.path().join("roots").join("current");
        let file = directory.path().join("data");
        std::fs::write(&file, b"someone's").unwrap();
        let made = runtime()
            .block_on(make_links(vec![
                (link.clone(), "/nix/store/old".to_owned()),
                (link.clone(), "/nix/store/new".to_owned()),
            ]))
            .unwrap();
        assert_eq!(made, vec![link.clone(), link.clone()]);
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            Path::new("/nix/store/new")
        );
        assert!(
            runtime()
                .block_on(make_links(vec![(
                    file.clone(),
                    "/nix/store/new".to_owned()
                )]))
                .is_err()
        );
        assert_eq!(std::fs::read(&file).unwrap(), b"someone's");
        let left: Vec<_> = std::fs::read_dir(link.parent().unwrap()).unwrap().collect();
        assert_eq!(left.len(), 1, "no temporary is left behind");
    }

    /// reads an `AddMultipleToStore` request to its end and answers it, returning how many NAR
    /// and header bytes the stream held.
    async fn take_add(stream: &mut UnixStream) -> u64 {
        assert_eq!(read_op(stream).await, op::ADD_MULTIPLE_TO_STORE);
        let mut flags = [0; 16];
        stream.read_exact(&mut flags).await.unwrap();
        let mut total = 0;
        loop {
            let mut word = [0; 8];
            stream.read_exact(&mut word).await.unwrap();
            let len = u64::from_le_bytes(word);
            if len == 0 {
                break;
            }
            let mut frame = vec![0; usize::try_from(len).unwrap()];
            stream.read_exact(&mut frame).await.unwrap();
            total += len;
        }
        let mut reply = Encoder::default();
        reply.u64(STDERR_LAST);
        stream.write_all(&reply.bytes).await.unwrap();
        total
    }

    fn source(name: &str, contents: &[u8], opening: crate::nar::Opening) -> SourceNar {
        let nar = NarNode::regular(false, contents.to_vec()).to_nar().unwrap();
        SourceNar {
            address: StoreDir::default()
                .source_address(
                    &shikumi_derivation::StoreName::new(name).unwrap(),
                    nar.hash,
                    shikumi_derivation::References::default(),
                )
                .unwrap(),
            nar_size: nar.bytes.len() as u64,
            nar: Box::new(Lazy::new(opening)),
        }
    }

    // a source's NAR opens on its first read, which for a NAR read from a daemon is a round trip.
    // here the first source can't open until the second has started opening, so the add finishes
    // only if later sources open while an earlier one streams; opened one after another, it hangs.
    #[test]
    fn a_later_source_opens_while_an_earlier_one_streams() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("socket");
        runtime().block_on(async {
            let listener = UnixListener::bind(&socket).unwrap();
            let daemon = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                greet(&mut stream).await;
                take_add(&mut stream).await
            });
            let store = WorkerStore::connect(
                Endpoint::Socket(socket.clone()),
                StoreDir::default(),
                NonZeroUsize::MIN,
            )
            .await
            .unwrap();
            let second_opening = Arc::new(tokio::sync::Notify::new());
            let (waits, signals) = (second_opening.clone(), second_opening);
            let first_bytes = NarNode::regular(false, b"first".to_vec())
                .to_nar()
                .unwrap()
                .bytes;
            let second_bytes = NarNode::regular(false, b"second".to_vec())
                .to_nar()
                .unwrap()
                .bytes;
            let sources = vec![
                source(
                    "first",
                    b"first",
                    Box::pin(async move {
                        waits.notified().await;
                        Ok(Box::new(io::Cursor::new(first_bytes)) as NarReader)
                    }),
                ),
                source(
                    "second",
                    b"second",
                    Box::pin(async move {
                        signals.notify_one();
                        Ok(Box::new(io::Cursor::new(second_bytes)) as NarReader)
                    }),
                ),
            ];
            tokio::time::timeout(Duration::from_secs(10), store.add_sources(sources))
                .await
                .expect("the second source opens while the first waits on it")
                .unwrap();
            assert!(daemon.await.unwrap() > 0);
        });
    }

    #[test]
    fn the_handshake_reports_the_negotiated_version_and_trust() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("socket");
        runtime().block_on(async {
            let listener = UnixListener::bind(&socket).unwrap();
            let daemon = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                greet(&mut stream).await;
            });
            let store = WorkerStore::connect(
                Endpoint::Socket(socket.clone()),
                StoreDir::default(),
                NonZeroUsize::MIN,
            )
            .await
            .unwrap();
            daemon.await.unwrap();
            assert_eq!(store.protocol(), "1.35");
            assert_eq!(store.daemon_version(), "2.95.4");
            assert_eq!(store.trust().await.unwrap(), Trust::Untrusted);
        });
    }

    // a build whose future is dropped, as a cancelled command's would be, must hang up on the
    // daemon: that hangup is what makes the daemon stop the build.
    #[test]
    fn a_dropped_build_closes_its_connection() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("socket");
        runtime().block_on(async {
            let listener = UnixListener::bind(&socket).unwrap();
            let daemon = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                greet(&mut stream).await;
                let op = read_op(&mut stream).await;
                // the build never finishes; this returns once the client hangs up.
                let mut rest = Vec::new();
                stream.read_to_end(&mut rest).await.unwrap();
                op
            });
            let store = WorkerStore::connect(
                Endpoint::Socket(socket.clone()),
                StoreDir::default(),
                NonZeroUsize::MIN,
            )
            .await
            .unwrap();
            let building =
                tokio::time::timeout(Duration::from_millis(100), store.build(&[target()])).await;
            assert!(building.is_err(), "the build can't have finished");
            let op = tokio::time::timeout(Duration::from_secs(10), daemon)
                .await
                .expect("the daemon sees the connection close once the build is dropped")
                .unwrap();
            assert_eq!(op, op::BUILD_PATHS_WITH_RESULTS);
        });
    }

    // the daemon's own words, and what it logged first, reach the caller; the connection that
    // carried the error isn't used again.
    #[test]
    fn a_daemon_error_reaches_the_caller_and_retires_its_connection() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("socket");
        runtime().block_on(async {
            let listener = UnixListener::bind(&socket).unwrap();
            let daemon = tokio::spawn(async move {
                let (mut first, _) = listener.accept().await.unwrap();
                greet(&mut first).await;
                assert_eq!(read_op(&mut first).await, op::QUERY_VALID_PATHS);
                let mut reply = Encoder::default();
                reply
                    .u64(STDERR_NEXT)
                    .string("checking paths\n")
                    .u64(STDERR_ERROR)
                    .string("Error")
                    .u64(0)
                    .string("Error")
                    .string("you are not privileged to do that")
                    .u64(0)
                    .u64(0);
                first.write_all(&reply.bytes).await.unwrap();
                let (mut second, _) = listener.accept().await.unwrap();
                greet(&mut second).await;
                assert_eq!(read_op(&mut second).await, op::QUERY_VALID_PATHS);
                let mut reply = Encoder::default();
                reply.u64(STDERR_LAST).u64(0);
                second.write_all(&reply.bytes).await.unwrap();
                // keep both open until the client is done, so a reuse of the first would hang
                // rather than fail on a closed socket.
                let mut rest = Vec::new();
                second.read_to_end(&mut rest).await.unwrap();
                drop(first);
            });
            let store = WorkerStore::connect(
                Endpoint::Socket(socket.clone()),
                StoreDir::default(),
                NonZeroUsize::MIN,
            )
            .await
            .unwrap();
            let error = store.query_valid(&[target().derivation]).await.unwrap_err();
            let message = error.to_string();
            assert!(
                message.contains("you are not privileged to do that"),
                "{message}"
            );
            assert!(message.contains("checking paths"), "{message}");
            let valid = tokio::time::timeout(
                Duration::from_secs(10),
                store.query_valid(&[target().derivation]),
            )
            .await
            .expect("the second query goes out on a new connection")
            .unwrap();
            assert!(valid.is_empty());
            store.close().await.unwrap();
            daemon.await.unwrap();
        });
    }
}
