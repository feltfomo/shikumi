//! the worker protocol's wire format: little-endian u64 words, length-prefixed strings padded to
//! eight bytes, counted lists, and the messages built from them.
//!
//! the layouts follow CppNix 2.35.3 `src/libutil/serialise.cc`, `src/libstore/common-protocol.cc`
//! and `src/libstore/worker-protocol.cc`, which Lix 2.95.4 and 2.96.0 share at protocol 1.35
//! (`lix/libstore/worker-protocol.cc`). the tests decode and encode the byte files CppNix 2.35.3
//! keeps in `src/libstore-tests/data/worker-protocol/`, so a drift shows without a daemon.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use serde::Deserialize;
use shikumi_derivation::{ContentAddress, Sha256, StoreDir, StorePath};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::{PathInfo, TransportError};

pub(crate) const MAGIC_CLIENT: u64 = 0x6e69_7863;
pub(crate) const MAGIC_DAEMON: u64 = 0x6478_696f;
/// what Determinate Nix 3.24.2 sends instead of its greeting to a user `allowed-users` leaves out
/// (`daemonLoop` in its `src/nix/unix/daemon.cc`); CppNix and Lix just close the connection.
pub(crate) const MAGIC_ACCESS_DENIED: u64 = 0xab9a_9ff0;

/// the newest minor version this client speaks. 1.38 adds a feature exchange that nothing here
/// needs, so CppNix and Determinate Nix negotiate 1.37 and Lix, frozen at 1.35, negotiates that.
pub(crate) const OFFERED_MINOR: u8 = 37;
/// Lix refuses clients below 1.35, and 1.35 is the first version whose handshake reports trust,
/// so nothing older is worth a code path.
pub(crate) const MINIMUM_MINOR: u8 = 35;

pub(crate) const STDERR_NEXT: u64 = 0x6f6c_6d67;
pub(crate) const STDERR_READ: u64 = 0x6461_7461;
pub(crate) const STDERR_WRITE: u64 = 0x6461_7416;
pub(crate) const STDERR_LAST: u64 = 0x616c_7473;
pub(crate) const STDERR_ERROR: u64 = 0x6378_7470;
pub(crate) const STDERR_START_ACTIVITY: u64 = 0x5354_5254;
pub(crate) const STDERR_STOP_ACTIVITY: u64 = 0x5354_4f50;
pub(crate) const STDERR_RESULT: u64 = 0x5253_4c54;

/// `resBuildLogLine`, the activity result that carries one line of a builder's output.
pub(crate) const RESULT_BUILD_LOG_LINE: u64 = 101;

/// the operations this client sends, numbered as in `WorkerProto::Op`.
pub(crate) mod op {
    pub(crate) const ADD_INDIRECT_ROOT: u64 = 12;
    pub(crate) const QUERY_PATH_INFO: u64 = 26;
    pub(crate) const QUERY_VALID_PATHS: u64 = 31;
    pub(crate) const NAR_FROM_PATH: u64 = 38;
    pub(crate) const ADD_MULTIPLE_TO_STORE: u64 = 44;
    pub(crate) const BUILD_PATHS_WITH_RESULTS: u64 = 46;
}

/// a string longer than this is a broken stream, not a message: nothing the daemon sends as one
/// string comes near it, and reading the length as given would allocate whatever it says.
const MAX_STRING: u64 = 64 << 20;

pub(crate) fn version_word(minor: u8) -> u64 {
    1 << 8 | u64::from(minor)
}

/// builds a request in memory before it's written, so the bytes can be checked without a socket.
#[derive(Debug, Default)]
pub(crate) struct Encoder {
    pub(crate) bytes: Vec<u8>,
}

impl Encoder {
    pub(crate) fn u64(&mut self, value: u64) -> &mut Self {
        self.bytes.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub(crate) fn bytes(&mut self, bytes: &[u8]) -> &mut Self {
        self.u64(bytes.len() as u64);
        self.bytes.extend_from_slice(bytes);
        let padding = (8 - bytes.len() % 8) % 8;
        self.bytes.extend_from_slice(&[0; 8][..padding]);
        self
    }

    pub(crate) fn string(&mut self, text: &str) -> &mut Self {
        self.bytes(text.as_bytes())
    }

    pub(crate) fn strings<'a>(
        &mut self,
        items: impl ExactSizeIterator<Item = &'a str>,
    ) -> &mut Self {
        self.u64(items.len() as u64);
        for item in items {
            self.string(item);
        }
        self
    }

    pub(crate) fn path(&mut self, dir: &StoreDir, path: &StorePath) -> &mut Self {
        self.string(&dir.display(path))
    }

    pub(crate) fn paths<'a>(
        &mut self,
        dir: &StoreDir,
        paths: impl ExactSizeIterator<Item = &'a StorePath>,
    ) -> &mut Self {
        self.u64(paths.len() as u64);
        for path in paths {
            self.path(dir, path);
        }
        self
    }

    /// a `ValidPathInfo` in its 1.16 layout, which is how `AddMultipleToStore` frames each path on
    /// every implementation. Shikumi sends no deriver, registration time or signatures, and never
    /// claims `ultimate`: the daemon clears it for an untrusted client anyway.
    pub(crate) fn path_info<'a>(
        &mut self,
        dir: &StoreDir,
        path: &StorePath,
        references: impl ExactSizeIterator<Item = &'a StorePath>,
        nar: (&Sha256, u64),
        content_address: ContentAddress,
    ) -> &mut Self {
        let (nar_hash, nar_size) = nar;
        self.path(dir, path)
            .string("")
            .string(&nar_hash.to_hex())
            .paths(dir, references)
            .u64(0)
            .u64(nar_size)
            .u64(0)
            .u64(0)
            .string(&content_address.to_string())
    }
}

/// a build's status as `BuildResult` carries it, numbered as CppNix 2.35.3's
/// `buildResultStatusTable` in `src/libstore/common-protocol.cc` and Lix's `BuildResult::Status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuildStatus {
    Built,
    Substituted,
    AlreadyValid,
    PermanentFailure,
    InputRejected,
    OutputRejected,
    TransientFailure,
    CachedFailure,
    TimedOut,
    MiscFailure,
    DependencyFailed,
    LogLimitExceeded,
    NotDeterministic,
    ResolvesToAlreadyValid,
    NoSubstituters,
    /// Determinate Nix 3.24.2 only.
    Cancelled,
    /// a status no implementation here sends, kept as its number for the failure message.
    Unknown(u64),
}

impl BuildStatus {
    fn from_wire(word: u64) -> Self {
        match word {
            0 => Self::Built,
            1 => Self::Substituted,
            2 => Self::AlreadyValid,
            3 => Self::PermanentFailure,
            4 => Self::InputRejected,
            5 => Self::OutputRejected,
            6 => Self::TransientFailure,
            7 => Self::CachedFailure,
            8 => Self::TimedOut,
            9 => Self::MiscFailure,
            10 => Self::DependencyFailed,
            11 => Self::LogLimitExceeded,
            12 => Self::NotDeterministic,
            13 => Self::ResolvesToAlreadyValid,
            14 => Self::NoSubstituters,
            16 => Self::Cancelled,
            other => Self::Unknown(other),
        }
    }

    pub(crate) fn succeeded(self) -> bool {
        matches!(
            self,
            Self::Built | Self::Substituted | Self::AlreadyValid | Self::ResolvesToAlreadyValid
        )
    }
}

impl fmt::Display for BuildStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(word) => write!(f, "status {word}"),
            known => write!(f, "{known:?}"),
        }
    }
}

/// one build's outcome as `BuildResult` carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BuildResult {
    pub(crate) status: BuildStatus,
    pub(crate) error: String,
    pub(crate) outputs: BTreeMap<String, StorePath>,
}

/// the one field of a realisation this client reads. the rest, such as its signatures, is left
/// unread rather than refused: this is Nix's JSON, not a contract Shikumi publishes, and each
/// implementation writes its own set of fields.
#[derive(Deserialize)]
struct Realisation {
    #[serde(rename = "outPath")]
    out_path: String,
}

/// reads messages from the daemon. every failure names the endpoint, since a pool can hold
/// connections to more than one.
#[derive(Debug)]
pub(crate) struct Decoder<R> {
    reader: R,
    endpoint: Arc<str>,
}

impl<R: AsyncRead + Unpin> Decoder<R> {
    pub(crate) fn new(reader: R, endpoint: Arc<str>) -> Self {
        Self { reader, endpoint }
    }

    pub(crate) fn endpoint(&self) -> &Arc<str> {
        &self.endpoint
    }

    pub(crate) fn reader(&mut self) -> &mut R {
        &mut self.reader
    }

    pub(crate) fn protocol(&self, detail: impl Into<String>) -> TransportError {
        TransportError::Protocol {
            endpoint: self.endpoint.to_string(),
            detail: detail.into(),
        }
    }

    async fn exact(&mut self, buf: &mut [u8]) -> Result<(), TransportError> {
        self.reader
            .read_exact(buf)
            .await
            .map(|_| ())
            .map_err(|source| TransportError::Connection {
                endpoint: self.endpoint.to_string(),
                source,
            })
    }

    pub(crate) async fn u64(&mut self) -> Result<u64, TransportError> {
        let mut word = [0; 8];
        self.exact(&mut word).await?;
        Ok(u64::from_le_bytes(word))
    }

    pub(crate) async fn bool(&mut self) -> Result<bool, TransportError> {
        match self.u64().await? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(self.protocol(format!("{other} where a boolean belongs"))),
        }
    }

    pub(crate) async fn bytes(&mut self) -> Result<Vec<u8>, TransportError> {
        let len = self.u64().await?;
        if len > MAX_STRING {
            return Err(self.protocol(format!("a string of {len} bytes")));
        }
        let len = usize::try_from(len).expect("a length under the cap fits in usize");
        let mut out = vec![0; len];
        self.exact(&mut out).await?;
        let mut padding = [0; 8];
        let padding = &mut padding[..(8 - len % 8) % 8];
        self.exact(padding).await?;
        if padding.iter().any(|&b| b != 0) {
            return Err(self.protocol("non-zero padding after a string"));
        }
        Ok(out)
    }

    pub(crate) async fn string(&mut self) -> Result<String, TransportError> {
        let bytes = self.bytes().await?;
        String::from_utf8(bytes).map_err(|_| self.protocol("a string that isn't UTF-8"))
    }

    /// a string meant for people, such as a log line or an error, where a stray byte shouldn't
    /// fail the operation it describes. daemons colour the paths and names in their messages
    /// whatever the client is, so the colour codes are taken out.
    pub(crate) async fn text(&mut self) -> Result<String, TransportError> {
        Ok(plain(&String::from_utf8_lossy(&self.bytes().await?)))
    }

    pub(crate) async fn count(&mut self) -> Result<usize, TransportError> {
        let count = self.u64().await?;
        // every element takes at least one word, so a count beyond the cap is a broken stream.
        if count > MAX_STRING {
            return Err(self.protocol(format!("a list of {count} elements")));
        }
        Ok(usize::try_from(count).expect("a count under the cap fits in usize"))
    }

    /// reads past a string without keeping it, for fields this client has no use for.
    pub(crate) async fn skip(&mut self) -> Result<(), TransportError> {
        let len = self.u64().await?;
        let mut left = len.div_ceil(8) * 8;
        let mut scratch = [0; 256];
        while left > 0 {
            let take = usize::try_from(left.min(scratch.len() as u64)).expect("at most 256");
            self.exact(&mut scratch[..take]).await?;
            left -= take as u64;
        }
        Ok(())
    }

    pub(crate) async fn skip_list(&mut self) -> Result<(), TransportError> {
        for _ in 0..self.count().await? {
            self.skip().await?;
        }
        Ok(())
    }

    pub(crate) async fn path(&mut self, dir: &StoreDir) -> Result<StorePath, TransportError> {
        let text = self.string().await?;
        dir.parse(&text)
            .map_err(|error| self.protocol(error.to_string()))
    }

    pub(crate) async fn optional_path(
        &mut self,
        dir: &StoreDir,
    ) -> Result<Option<StorePath>, TransportError> {
        let text = self.string().await?;
        if text.is_empty() {
            return Ok(None);
        }
        dir.parse(&text)
            .map(Some)
            .map_err(|error| self.protocol(error.to_string()))
    }

    pub(crate) async fn paths(
        &mut self,
        dir: &StoreDir,
    ) -> Result<BTreeSet<StorePath>, TransportError> {
        let count = self.count().await?;
        let mut out = BTreeSet::new();
        for _ in 0..count {
            out.insert(self.path(dir).await?);
        }
        Ok(out)
    }

    /// an `UnkeyedValidPathInfo` at 1.16 or later, for a path the caller already knows.
    pub(crate) async fn path_info(
        &mut self,
        dir: &StoreDir,
        path: StorePath,
    ) -> Result<PathInfo, TransportError> {
        let deriver = self.optional_path(dir).await?;
        let hex = self.string().await?;
        let nar_hash = Sha256::parse_hex(&hex).map_err(|error| self.protocol(error.to_string()))?;
        let references = self.paths(dir).await?;
        let _registration_time = self.u64().await?;
        let nar_size = self.u64().await?;
        let _ultimate = self.bool().await?;
        // signatures and the content address: the address is computed, never taken from here.
        self.skip_list().await?;
        self.skip().await?;
        Ok(PathInfo {
            path,
            nar_hash,
            nar_size,
            references,
            deriver,
        })
    }

    /// a `BuildResult` at `minor`, 1.29 or later. its outputs come as a map from an output id to a
    /// realisation in JSON, because this client doesn't negotiate the 1.38 feature that replaces
    /// it; CppNix writes the id with a dummy hash and Lix with the real one, and both end it in
    /// `!<output>` and give `outPath` as a base name.
    pub(crate) async fn build_result(
        &mut self,
        dir: &StoreDir,
        minor: u8,
    ) -> Result<BuildResult, TransportError> {
        let status = BuildStatus::from_wire(self.u64().await?);
        let error = self.text().await?;
        let _times_built = self.u64().await?;
        let _non_deterministic = self.bool().await?;
        let _start_time = self.u64().await?;
        let _stop_time = self.u64().await?;
        if minor >= 37 {
            for _ in ["cpuUser", "cpuSystem"] {
                match self.u64().await? {
                    0 => {}
                    1 => {
                        self.u64().await?;
                    }
                    tag => return Err(self.protocol(format!("optional duration tagged {tag}"))),
                }
            }
        }
        let count = self.count().await?;
        let mut outputs = BTreeMap::new();
        for _ in 0..count {
            let id = self.string().await?;
            let realisation = self.string().await?;
            let output = id
                .rsplit_once('!')
                .map(|(_, output)| output.to_owned())
                .ok_or_else(|| self.protocol(format!("output id '{id}' names no output")))?;
            let Realisation { out_path } = serde_json::from_str(&realisation)
                .map_err(|error| self.protocol(format!("realisation of '{id}': {error}")))?;
            let path = match out_path.strip_prefix('/') {
                Some(_) => dir.parse(&out_path),
                None => StorePath::from_base_name(&out_path),
            }
            .map_err(|error| self.protocol(error.to_string()))?;
            outputs.insert(output, path);
        }
        Ok(BuildResult {
            status,
            error,
            outputs,
        })
    }

    /// the payload of `STDERR_ERROR` at 1.26 or later, as one message with its traces.
    pub(crate) async fn error(&mut self) -> Result<String, TransportError> {
        let kind = self.text().await?;
        if kind != "Error" {
            return Err(self.protocol(format!("an error of type '{kind}'")));
        }
        let _level = self.u64().await?;
        let _name = self.text().await?;
        let mut message = self.text().await?;
        if self.u64().await? != 0 {
            return Err(self.protocol("an error with a position, which no daemon sends"));
        }
        for _ in 0..self.count().await? {
            if self.u64().await? != 0 {
                return Err(self.protocol("an error trace with a position"));
            }
            let trace = self.text().await?;
            message.push_str("\n… ");
            message.push_str(&trace);
        }
        Ok(message)
    }

    /// activity fields, keeping the first string one, which is the line a build log result holds.
    pub(crate) async fn fields(&mut self) -> Result<Option<String>, TransportError> {
        let mut first = None;
        for _ in 0..self.count().await? {
            match self.u64().await? {
                0 => {
                    self.u64().await?;
                }
                1 => {
                    let text = self.text().await?;
                    first.get_or_insert(text);
                }
                kind => return Err(self.protocol(format!("an activity field of type {kind}"))),
            }
        }
        Ok(first)
    }
}

/// `text` without its terminal control sequences: `ESC [`, parameters, then a final byte from `@`
/// to `~`.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.next() == Some('[') {
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    fn decoder(bytes: &[u8]) -> Decoder<&[u8]> {
        Decoder::new(bytes, Arc::from("test"))
    }

    fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn path(base: &str) -> StorePath {
        StorePath::from_base_name(base).unwrap()
    }

    // `string.bin`: "", "hi", "white rabbit", "大白兔", "oh no " written one after another.
    const STRINGS: &str = "0000000000000000020000000000000068690000000000000c00000000000000776869746520726162626974000000000900000000000000e5a4a7e799bde585940000000000000006000000000000006f68206e6f200000";

    #[test]
    fn strings_encode_and_decode_as_cppnix_writes_them() {
        let texts = ["", "hi", "white rabbit", "大白兔", "oh no "];
        let mut encoder = Encoder::default();
        for text in texts {
            encoder.string(text);
        }
        assert_eq!(encoder.bytes, unhex(STRINGS));
        let bytes = unhex(STRINGS);
        let mut decoder = decoder(&bytes);
        for text in texts {
            assert_eq!(block_on(decoder.string()).unwrap(), text);
        }
    }

    #[test]
    fn a_string_with_non_zero_padding_is_refused() {
        let mut bytes = unhex(STRINGS);
        // the last byte of the padding after "hi".
        bytes[23] = 1;
        let mut decoder = decoder(&bytes);
        block_on(decoder.string()).unwrap();
        assert!(matches!(
            block_on(decoder.string()),
            Err(TransportError::Protocol { .. })
        ));
    }

    // `handshake-to-client.bin`: the daemon's greeting and its version, 1.10 in that test.
    #[test]
    fn the_daemon_greeting_reads_as_its_magic_and_version() {
        let bytes = unhex("6f697864000000000a01000000000000");
        let mut decoder = decoder(&bytes);
        assert_eq!(block_on(decoder.u64()).unwrap(), MAGIC_DAEMON);
        assert_eq!(block_on(decoder.u64()).unwrap(), version_word(10));
    }

    // `client-handshake-info_1_35.bin`: version "foo" untrusted, then "bar" trusted, as the daemon
    // ends the handshake at 1.35.
    #[test]
    fn handshake_info_carries_the_version_and_the_trust_word() {
        let bytes = unhex(
            "0300000000000000666f6f00000000000200000000000000030000000000000062617200000000000100000000000000",
        );
        let mut decoder = decoder(&bytes);
        assert_eq!(block_on(decoder.string()).unwrap(), "foo");
        assert_eq!(block_on(decoder.u64()).unwrap(), 2);
        assert_eq!(block_on(decoder.string()).unwrap(), "bar");
        assert_eq!(block_on(decoder.u64()).unwrap(), 1);
    }

    // the last entry of `derived-path-1.30.bin`: outputs `x` and `y` of `bar.drv`, which is the
    // form `BuildPathsWithResults` takes a target in.
    #[test]
    fn a_derived_path_is_the_drv_and_its_outputs_after_a_bang() {
        let mut encoder = Encoder::default();
        encoder.string("/nix/store/g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv!x,y");
        assert_eq!(
            encoder.bytes,
            unhex(
                "37000000000000002f6e69782f73746f72652f67317737687933716731773768793371673177376879337167317737687933712d6261722e64727621782c7900"
            )
        );
    }

    // `valid-path-info-1.16.bin`: three `ValidPathInfo`s, the last content-addressed with a
    // reference to itself.
    const PATH_INFOS: &str = "2f000000000000002f6e69782f73746f72652f67317737687933716731773768793371673177376879337167317737687933712d62617200000000000000000040000000000000003135653363353630383934636262323730383563663635623561326563623138343838633939393439376634353331623639303761373538316365366435323700000000000000007f5b0000000000003e880000000000000100000000000000000000000000000000000000000000002f000000000000002f6e69782f73746f72652f67317737687933716731773768793371673177376879337167317737687933712d6261720033000000000000002f6e69782f73746f72652f67317737687933716731773768793371673177376879337167317737687933712d6261722e647276000000000040000000000000003135653363353630383934636262323730383563663635623561326563623138343838633939393439376634353331623639303761373538316365366435323702000000000000002f000000000000002f6e69782f73746f72652f67317737687933716731773768793371673177376879337167317737687933712d626172002f000000000000002f6e69782f73746f72652f67317737687979797931773768793371673177376879337167717171717933712d666f6f007f5b0000000000003e8800000000000000000000000000000200000000000000630000000000000066616b652d7369672d313a41414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141413d3d0000000000630000000000000066616b652d7369672d323a41414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141414141413d3d000000000000000000000000002f000000000000002f6e69782f73746f72652f6e35776b643966727234357061373469663567707a396a376d696667323766682d666f6f00000000000000000040000000000000003135653363353630383934636262323730383563663635623561326563623138343838633939393439376634353331623639303761373538316365366435323702000000000000002f000000000000002f6e69782f73746f72652f67317737687933716731773768793371673177376879337167317737687933712d626172002f000000000000002f6e69782f73746f72652f6e35776b643966727234357061373469663567707a396a376d696667323766682d666f6f007f5b0000000000003e8800000000000000000000000000000000000000000000430000000000000066697865643a723a7368613235363a316c72313837763664636b31726a68326a36737670696b63667a353377796c3371726c6362623430357a6c68313378306b6868680000000000";
    const NAR_HASH: &str = "15e3c560894cbb27085cf65b5a2ecb18488c999497f4531b6907a7581ce6d527";
    const CONTENT_ADDRESS: &str =
        "fixed:r:sha256:1lr187v6dck1rjh2j6svpikcfz53wyl3qrlcbb405zlh13x0khhh";

    #[test]
    fn path_infos_decode_with_derivers_references_and_signatures_skipped() {
        let dir = StoreDir::default();
        let bytes = unhex(PATH_INFOS);
        let mut decoder = decoder(&bytes);
        let mut infos = Vec::new();
        for _ in 0..3 {
            let path = block_on(decoder.path(&dir)).unwrap();
            infos.push(block_on(decoder.path_info(&dir, path)).unwrap());
        }
        assert!(block_on(decoder.u64()).is_err(), "the file holds three");
        assert!(infos.iter().all(|info| info.nar_hash.to_hex() == NAR_HASH));
        assert!(infos.iter().all(|info| info.nar_size == 34878));
        assert_eq!(infos[0].deriver, None);
        assert!(infos[0].references.is_empty());
        assert_eq!(
            infos[1].deriver,
            Some(path("g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv"))
        );
        assert_eq!(
            infos[2].references,
            BTreeSet::from([
                path("g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar"),
                path("n5wkd9frr45pa74if5gpz9j7mifg27fh-foo"),
            ])
        );
    }

    // the third entry has no deriver, no signatures and isn't ultimate, which is everything
    // `AddMultipleToStore` sends, so its bytes are exactly what this encoder writes.
    #[test]
    fn a_path_to_add_encodes_as_cppnix_writes_a_content_addressed_path_info() {
        let dir = StoreDir::default();
        let references = BTreeSet::from([
            path("g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar"),
            path("n5wkd9frr45pa74if5gpz9j7mifg27fh-foo"),
        ]);
        // the hash in the file's content address, `sha256-EMIJ+giQ/gLIWoxmPKjno3zHZrxbGymgzGGyZvZBIdM=`
        // in its JSON.
        let content =
            Sha256::parse_hex("10c209fa0890fe02c85a8c663ca8e7a37cc766bc5b1b29a0cc61b266f64121d3")
                .unwrap();
        let mut encoder = Encoder::default();
        encoder.path_info(
            &dir,
            &path("n5wkd9frr45pa74if5gpz9j7mifg27fh-foo"),
            references.iter(),
            (&Sha256::parse_hex(NAR_HASH).unwrap(), 34878),
            ContentAddress::Recursive(content),
        );
        // the encoder writes registration time 0 where the file has 23423 (0x5b7f).
        let mut expected = unhex(PATH_INFOS);
        let expected = expected.split_off(expected.len() - encoder.bytes.len());
        let mut written = encoder.bytes;
        let time = written.len() - 8 * 3 - (8 + CONTENT_ADDRESS.len().div_ceil(8) * 8) - 8;
        written[time..time + 2].copy_from_slice(&[0x7f, 0x5b]);
        assert_eq!(written, expected);
    }

    // `build-result-1.37.bin`: two failures and a success with outputs `bar` and `foo`.
    const BUILD_RESULTS: &str = "05000000000000000b000000000000006e6f206964656120776879000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000c000000000000000b000000000000006e6f2069646561207768790000000000030000000000000001000000000000001e00000000000000320000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000001e00000000000000320000000000000001000000000000000065cd1d000000000100000000000000004f00240000000002000000000000004b000000000000007368613235363a36663836396639656132383233626461313635653036303736666430646534333636646561643263306538643264626261643237376434663135633337336635216261720000000000b0000000000000007b22646570656e64656e745265616c69736174696f6e73223a7b7d2c226964223a227368613235363a3666383639663965613238323362646131363565303630373666643064653433363664656164326330653864326462626164323737643466313563333733663521626172222c226f757450617468223a2267317737687933716731773768793371673177376879337167317737687933712d626172222c227369676e617475726573223a5b5d7d4b000000000000007368613235363a3666383639663965613238323362646131363565303630373666643064653433363664656164326330653864326462626164323737643466313563333733663521666f6f0000000000b0000000000000007b22646570656e64656e745265616c69736174696f6e73223a7b7d2c226964223a227368613235363a3666383639663965613238323362646131363565303630373666643064653433363664656164326330653864326462626164323737643466313563333733663521666f6f222c226f757450617468223a2267317737687933716731773768793371673177376879337167317737687933712d666f6f222c227369676e617475726573223a5b5d7d";

    #[test]
    fn build_results_at_1_37_carry_cpu_times_and_outputs() {
        let dir = StoreDir::default();
        let bytes = unhex(BUILD_RESULTS);
        let mut decoder = decoder(&bytes);
        let results: Vec<BuildResult> = (0..3)
            .map(|_| block_on(decoder.build_result(&dir, 37)).unwrap())
            .collect();
        assert!(block_on(decoder.u64()).is_err(), "the file holds three");
        assert_eq!(results[0].status, BuildStatus::OutputRejected);
        assert_eq!(results[0].status.to_string(), "OutputRejected");
        assert_eq!(results[0].error, "no idea why");
        assert!(!results[1].status.succeeded());
        assert_eq!(results[1].status, BuildStatus::NotDeterministic);
        assert!(results[2].status.succeeded());
        assert_eq!(
            results[2].outputs,
            BTreeMap::from([
                (
                    "bar".to_owned(),
                    path("g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar")
                ),
                (
                    "foo".to_owned(),
                    path("g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo")
                ),
            ])
        );
    }

    // `keyed-build-result-1.29.bin`: below 1.37 there are no CPU times, so reading the file at
    // 1.37 would misread it.
    #[test]
    fn keyed_build_results_below_1_37_have_no_cpu_times() {
        let dir = StoreDir::default();
        let bytes = unhex(
            "2f000000000000002f6e69782f73746f72652f67317737687933716731773768793371673177376879337167317737687933712d7878780005000000000000000b000000000000006e6f20696465612077687900000000000000000000000000000000000000000000000000000000000000000000000000000000000000000037000000000000002f6e69782f73746f72652f67317737687933716731773768793371673177376879337167317737687933712d6261722e647276216f7574000c000000000000000b000000000000006e6f2069646561207768790000000000030000000000000001000000000000001e0000000000000032000000000000000000000000000000",
        );
        let mut decoder = decoder(&bytes);
        for key in [
            "/nix/store/g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-xxx",
            "/nix/store/g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv!out",
        ] {
            assert_eq!(block_on(decoder.string()).unwrap(), key);
            let result = block_on(decoder.build_result(&dir, 29)).unwrap();
            assert_eq!(result.error, "no idea why");
            assert!(result.outputs.is_empty());
        }
        assert!(block_on(decoder.u64()).is_err(), "the file holds two");
    }

    // as Lix 2.95.4 refused an untrusted add in the conformance VM.
    #[test]
    fn colour_codes_are_taken_out_of_daemon_text() {
        let mut encoder = Encoder::default();
        encoder.string("cannot add path '\u{1b}[35;1m/nix/store/x\u{1b}[0m' because");
        assert_eq!(
            block_on(decoder(&encoder.bytes).text()).unwrap(),
            "cannot add path '/nix/store/x' because"
        );
        assert_eq!(plain("no codes"), "no codes");
    }

    #[test]
    fn a_status_no_implementation_sends_keeps_its_number_and_fails() {
        let status = BuildStatus::from_wire(99);
        assert!(!status.succeeded());
        assert_eq!(status.to_string(), "status 99");
    }

    #[test]
    fn an_error_reads_as_its_message_and_traces() {
        let mut encoder = Encoder::default();
        encoder
            .string("Error")
            .u64(0)
            .string("Error")
            .string("path is not valid")
            .u64(0)
            .u64(1)
            .u64(0)
            .string("while adding");
        let mut decoder = decoder(&encoder.bytes);
        assert_eq!(
            block_on(decoder.error()).unwrap(),
            "path is not valid\n… while adding"
        );
    }
}
