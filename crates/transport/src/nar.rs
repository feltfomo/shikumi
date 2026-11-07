//! readers of a valid path's NAR, shared by both transports: one that opens its source on the
//! first read, and one that holds the source to the size and hash the daemon recorded.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use sha2::Digest;
use shikumi_derivation::Sha256;
use tokio::io::{AsyncRead, ReadBuf};

use crate::{NarReader, PathInfo};

pub(crate) type Opening = Pin<Box<dyn Future<Output = io::Result<NarReader>> + Send>>;

/// a reader that starts its source, a process or a connection, only when it's first read, so a
/// batch of them costs nothing until each one's turn comes.
pub(crate) struct Lazy {
    state: State,
}

enum State {
    Opening(Opening),
    Reading(NarReader),
    Failed,
}

impl Lazy {
    pub(crate) fn new(opening: Opening) -> Self {
        Self {
            state: State::Opening(opening),
        }
    }
}

impl AsyncRead for Lazy {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            match &mut this.state {
                State::Opening(opening) => match ready!(opening.as_mut().poll(cx)) {
                    Ok(reader) => this.state = State::Reading(reader),
                    Err(error) => {
                        this.state = State::Failed;
                        return Poll::Ready(Err(error));
                    }
                },
                State::Reading(reader) => return Pin::new(reader).poll_read(cx, buf),
                State::Failed => {
                    return Poll::Ready(Err(io::Error::other(
                        "reading a NAR whose source already failed to open",
                    )));
                }
            }
        }
    }
}

/// passes a NAR through, failing at the end unless it was exactly the recorded size and hash. a
/// short or altered NAR would otherwise reach whatever consumes it, such as a re-add that the
/// daemon then refuses with a less useful message, or worse, a reader that trusts it.
pub(crate) struct Verified<R> {
    inner: R,
    path: String,
    expected_size: u64,
    expected_hash: Sha256,
    read: u64,
    hasher: sha2::Sha256,
    finished: bool,
}

impl<R> Verified<R> {
    pub(crate) fn new(inner: R, path: String, info: &PathInfo) -> Self {
        Self {
            inner,
            path,
            expected_size: info.nar_size,
            expected_hash: info.nar_hash,
            read: 0,
            hasher: sha2::Sha256::new(),
            finished: false,
        }
    }

    fn mismatch(&self, detail: String) -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the NAR of '{}' {detail}", self.path),
        )
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for Verified<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.finished {
            return Poll::Ready(Ok(()));
        }
        let before = buf.filled().len();
        ready!(Pin::new(&mut this.inner).poll_read(cx, buf))?;
        let fresh = &buf.filled()[before..];
        if fresh.is_empty() {
            this.finished = true;
            if this.read != this.expected_size {
                return Poll::Ready(Err(this.mismatch(format!(
                    "ended after {} of {} bytes",
                    this.read, this.expected_size
                ))));
            }
            let hash = Sha256::from_bytes(std::mem::take(&mut this.hasher).finalize().into());
            if hash != this.expected_hash {
                return Poll::Ready(Err(this.mismatch(format!(
                    "hashes to {}, not the recorded {}",
                    hash.to_hex(),
                    this.expected_hash.to_hex()
                ))));
            }
            return Poll::Ready(Ok(()));
        }
        this.read += fresh.len() as u64;
        if this.read > this.expected_size {
            this.finished = true;
            return Poll::Ready(Err(this.mismatch(format!(
                "runs past its recorded {} bytes",
                this.expected_size
            ))));
        }
        this.hasher.update(fresh);
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shikumi_derivation::{NarNode, StorePath};
    use tokio::io::AsyncReadExt;

    fn info(bytes: &[u8]) -> PathInfo {
        PathInfo {
            path: StorePath::from_base_name("0mdqa9w1p6cmli6976v4wi0sw9r4p5pr-a").unwrap(),
            nar_hash: Sha256::digest(bytes),
            nar_size: bytes.len() as u64,
            references: Default::default(),
            deriver: None,
        }
    }

    fn read_all(reader: impl AsyncRead + Unpin) -> io::Result<Vec<u8>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let mut reader = reader;
            let mut out = Vec::new();
            reader.read_to_end(&mut out).await.map(|_| out)
        })
    }

    #[test]
    fn a_nar_of_the_recorded_size_and_hash_passes_through() {
        let nar = NarNode::regular(false, b"x".to_vec()).to_nar().unwrap();
        let verified = Verified::new(&nar.bytes[..], "a".to_owned(), &info(&nar.bytes));
        assert_eq!(read_all(verified).unwrap(), nar.bytes);
    }

    #[test]
    fn a_short_nar_fails_at_its_end() {
        let nar = NarNode::regular(false, b"x".to_vec()).to_nar().unwrap();
        let short = &nar.bytes[..nar.bytes.len() - 8];
        let error = read_all(Verified::new(short, "a".to_owned(), &info(&nar.bytes))).unwrap_err();
        assert!(error.to_string().contains("ended after"), "{error}");
    }

    #[test]
    fn an_altered_nar_of_the_right_size_fails_on_its_hash() {
        let nar = NarNode::regular(false, b"x".to_vec()).to_nar().unwrap();
        let altered = NarNode::regular(false, b"y".to_vec()).to_nar().unwrap();
        let verified = Verified::new(&altered.bytes[..], "a".to_owned(), &info(&nar.bytes));
        let error = read_all(verified).unwrap_err();
        assert!(error.to_string().contains("hashes to"), "{error}");
    }

    #[test]
    fn a_lazy_reader_opens_only_when_read() {
        let opened = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = opened.clone();
        let lazy = Lazy::new(Box::pin(async move {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(Box::new(&b"bytes"[..]) as NarReader)
        }));
        assert!(!opened.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(read_all(lazy).unwrap(), b"bytes");
        assert!(opened.load(std::sync::atomic::Ordering::SeqCst));
    }
}
