//! Files from the phone to the computer, over the link: a file shared to
//! Shodh, added in the Library, or a photo to ask about.
//!
//! The file goes in parts on the link's own connection, as three bridge
//! calls the computer checks like any other ([`BEGIN`], [`CHUNK`],
//! [`FINISH`]):
//!
//! - **Begin** names the file, its size, its SHA-256 and where it goes. The
//!   computer applies its upload rules before a byte is sent (the name, files
//!   that may hold secrets, the size limits; `phone_link/files.rs`) and
//!   answers how much of it it already has, so a transfer that broke off
//!   resumes where it stopped, also after either side restarted.
//! - **Chunk** appends one part of at most [`CHUNK_BYTES`] at the offset the
//!   computer has; any other offset is refused and the phone begins again to
//!   learn where to go on from.
//! - **Finish** checks the whole file's size and SHA-256 before it is handed
//!   to the computer's normal upload path.
//!
//! The computer keeps parts in a staging folder ([`Staging`]), by phone: a
//! phone can only write to its own transfers, at most [`MAX_OPEN`] at once and
//! [`MAX_STAGED_BYTES`] in all, and parts left for [`STALE_MS`] are removed.

use std::fs::{self, File, OpenOptions};
use std::future::Future;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use data_encoding::{BASE64, HEXLOWER, HEXLOWER_PERMISSIVE};
use ring::digest::{Context, SHA256};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Begin (or resume) a transfer.
pub const BEGIN: &str = "phone_file_begin";
/// One part of a transfer.
pub const CHUNK: &str = "phone_file_chunk";
/// The whole file is there: check it and add it.
pub const FINISH: &str = "phone_file_finish";

/// The largest part sent in one call: well inside one 4 MiB frame as base64.
pub const CHUNK_BYTES: usize = 512 * 1024;

/// Transfers one phone may have open at once.
pub const MAX_OPEN: usize = 4;

/// Bytes the computer stages for one phone at once (the files' declared sizes).
pub const MAX_STAGED_BYTES: u64 = 1024 * 1024 * 1024;

/// Parts left this long are removed.
pub const STALE_MS: u64 = 24 * 60 * 60 * 1000;

/// What the file is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Purpose {
    /// Added to the library: "From my phone", or a workspace.
    Library,
    /// A photo to ask about: read, cited as "Your photo", and not added to the
    /// library unless the user keeps it.
    Photo,
}

/// The file the phone offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Offer {
    /// Its own name, as the computer keeps it.
    pub name: String,
    pub size: u64,
    /// SHA-256 of the whole file, hex.
    pub sha256: String,
    /// The workspace it goes to; `None`: "From my phone".
    #[serde(default)]
    pub workspace_id: Option<String>,
    pub purpose: Purpose,
}

/// The computer's answer to [`BEGIN`] and [`CHUNK`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub transfer_id: String,
    /// Bytes the computer has.
    pub received: u64,
}

/// One part, as [`CHUNK`] sends it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    pub transfer_id: String,
    pub offset: u64,
    /// The bytes, in base64.
    pub data: String,
}

/// Why the computer refused a transfer call, in plain words.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransferError {
    #[error("This file is not on its way to your computer any more. Send it again.")]
    Unknown,
    #[error("Your computer has {expected} bytes of this file; the phone goes on from there.")]
    OutOfOrder { expected: u64 },
    #[error("This part is longer than the file.")]
    TooLong,
    #[error("The file changed on its way to your computer. Send it again.")]
    Changed,
    #[error("The file is not complete yet.")]
    Incomplete,
    #[error("Your computer is already receiving {MAX_OPEN} files from this phone. Wait until they are in.")]
    Busy,
    #[error(
        "Your computer is already receiving 1 GB from this phone. Wait until those files are in."
    )]
    Full,
    #[error("This file's details are not valid.")]
    Invalid,
    #[error("Your computer could not keep the file: {0}")]
    Io(String),
}

impl TransferError {
    /// The error as the phone receives it: `{ code, message }`, with
    /// `received` for a part sent at the wrong offset.
    pub fn to_value(&self) -> Value {
        match self {
            TransferError::OutOfOrder { expected } => json!({
                "code": "out_of_order",
                "message": self.to_string(),
                "received": expected,
            }),
            TransferError::Io(_) => json!({ "code": "runtime_error", "message": self.to_string() }),
            _ => json!({ "code": "refused", "message": self.to_string() }),
        }
    }
}

fn io(e: std::io::Error) -> TransferError {
    TransferError::Io(e.to_string())
}

/// SHA-256 of `reader`, hex.
pub fn sha256_hex(mut reader: impl Read) -> std::io::Result<String> {
    let mut context = Context::new(&SHA256);
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        context.update(&buffer[..read]);
    }
    Ok(HEXLOWER.encode(context.finish().as_ref()))
}

/// What the computer keeps of one transfer next to its part.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    node_id: String,
    offer: Offer,
    created_ms: u64,
}

/// A transfer whose bytes are all there and checked.
#[derive(Debug)]
pub struct Arrived {
    pub transfer_id: String,
    pub offer: Offer,
    /// The complete file, still in the staging folder: move it, then
    /// [`Staging::forget`] the transfer.
    pub path: PathBuf,
}

/// The computer's staging folder for files coming from phones.
pub struct Staging {
    dir: PathBuf,
}

impl Staging {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn part(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.part"))
    }

    fn meta_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    fn read_meta(&self, id: &str) -> Option<Meta> {
        let bytes = fs::read(self.meta_path(id)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Every transfer kept, with its id.
    fn all(&self) -> Vec<(String, Meta)> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let id = name.strip_suffix(".json")?.to_string();
                self.read_meta(&id).map(|m| (id, m))
            })
            .collect()
    }

    /// The transfer `id` of `node_id`; another phone's is unknown to this one.
    fn own(&self, node_id: &str, id: &str) -> Result<Meta, TransferError> {
        if !is_id(id) {
            return Err(TransferError::Unknown);
        }
        match self.read_meta(id) {
            Some(meta) if meta.node_id == node_id => Ok(meta),
            _ => Err(TransferError::Unknown),
        }
    }

    /// Remove transfers left for [`STALE_MS`].
    pub fn sweep(&self, now_ms: u64) {
        for (id, meta) in self.all() {
            if now_ms.saturating_sub(meta.created_ms) > STALE_MS {
                self.forget(&id);
            }
        }
    }

    /// Begin, or resume, `offer` from `node_id`: its id and the bytes the
    /// computer already has. The caller has applied the upload rules.
    pub fn begin(
        &self,
        node_id: &str,
        offer: &Offer,
        now_ms: u64,
    ) -> Result<Progress, TransferError> {
        let sha = offer.sha256.to_ascii_lowercase();
        if offer.size == 0
            || sha.len() != 64
            || HEXLOWER_PERMISSIVE.decode(sha.as_bytes()).is_err()
            || offer.name.trim().is_empty()
        {
            return Err(TransferError::Invalid);
        }
        fs::create_dir_all(&self.dir).map_err(io)?;
        self.sweep(now_ms);
        let id = transfer_id(node_id, offer);
        if let Some(meta) = self.read_meta(&id) {
            if meta.node_id == node_id {
                let received = self.received(&id, offer.size)?;
                return Ok(Progress {
                    transfer_id: id,
                    received,
                });
            }
        }
        let mine: Vec<Meta> = self
            .all()
            .into_iter()
            .map(|(_, m)| m)
            .filter(|m| m.node_id == node_id)
            .collect();
        if mine.len() >= MAX_OPEN {
            return Err(TransferError::Busy);
        }
        let staged: u64 = mine.iter().map(|m| m.offer.size).sum();
        if staged.saturating_add(offer.size) > MAX_STAGED_BYTES {
            return Err(TransferError::Full);
        }
        let meta = Meta {
            node_id: node_id.to_string(),
            offer: Offer {
                sha256: sha,
                ..offer.clone()
            },
            created_ms: now_ms,
        };
        let bytes = serde_json::to_vec(&meta).map_err(|e| TransferError::Io(e.to_string()))?;
        File::create(self.part(&id)).map_err(io)?;
        write_synced(&self.meta_path(&id), &bytes)?;
        Ok(Progress {
            transfer_id: id,
            received: 0,
        })
    }

    /// Bytes of transfer `id` on disk; a part longer than the file is started again.
    fn received(&self, id: &str, size: u64) -> Result<u64, TransferError> {
        let part = self.part(id);
        let len = match fs::metadata(&part) {
            Ok(m) => m.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                File::create(&part).map_err(io)?;
                0
            }
            Err(e) => return Err(io(e)),
        };
        if len > size {
            File::create(&part).map_err(io)?;
            return Ok(0);
        }
        Ok(len)
    }

    /// Append `part` from `node_id` at the offset the computer has.
    pub fn chunk(&self, node_id: &str, part: &Part) -> Result<Progress, TransferError> {
        let meta = self.own(node_id, &part.transfer_id)?;
        let data = BASE64
            .decode(part.data.as_bytes())
            .map_err(|_| TransferError::Invalid)?;
        if data.is_empty() || data.len() > CHUNK_BYTES {
            return Err(TransferError::Invalid);
        }
        let have = self.received(&part.transfer_id, meta.offer.size)?;
        if part.offset != have {
            return Err(TransferError::OutOfOrder { expected: have });
        }
        if have + data.len() as u64 > meta.offer.size {
            return Err(TransferError::TooLong);
        }
        let mut file = OpenOptions::new()
            .append(true)
            .open(self.part(&part.transfer_id))
            .map_err(io)?;
        file.write_all(&data).map_err(io)?;
        file.sync_data().map_err(io)?;
        Ok(Progress {
            transfer_id: part.transfer_id.clone(),
            received: have + data.len() as u64,
        })
    }

    /// The whole file of transfer `id`, checked against its size and SHA-256.
    /// A file that does not match is removed: it is sent again from the start.
    pub fn finish(&self, node_id: &str, id: &str) -> Result<Arrived, TransferError> {
        let meta = self.own(node_id, id)?;
        let have = self.received(id, meta.offer.size)?;
        if have != meta.offer.size {
            return Err(TransferError::Incomplete);
        }
        let path = self.part(id);
        let sha = sha256_hex(File::open(&path).map_err(io)?).map_err(io)?;
        if sha != meta.offer.sha256 {
            self.forget(id);
            return Err(TransferError::Changed);
        }
        Ok(Arrived {
            transfer_id: id.to_string(),
            offer: meta.offer,
            path,
        })
    }

    /// Remove transfer `id`'s part and record.
    pub fn forget(&self, id: &str) {
        if !is_id(id) {
            return;
        }
        let _ = fs::remove_file(self.part(id));
        let _ = fs::remove_file(self.meta_path(id));
    }
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), TransferError> {
    let mut file = File::create(path).map_err(io)?;
    file.write_all(bytes).map_err(io)?;
    file.sync_all().map_err(io)
}

/// A transfer's id: the same for the same file from the same phone to the same
/// place, so a transfer begun again resumes. Hex, so it is a safe file name.
pub fn transfer_id(node_id: &str, offer: &Offer) -> String {
    let mut context = Context::new(&SHA256);
    for part in [
        node_id,
        offer.name.as_str(),
        &offer.size.to_string(),
        &offer.sha256.to_ascii_lowercase(),
        offer.workspace_id.as_deref().unwrap_or(""),
        match offer.purpose {
            Purpose::Library => "library",
            Purpose::Photo => "photo",
        },
    ] {
        context.update(&(part.len() as u64).to_be_bytes());
        context.update(part.as_bytes());
    }
    HEXLOWER.encode(&context.finish().as_ref()[..16])
}

fn is_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// The computer, as the phone's sender reaches it: one allowlisted call.
pub trait Peer {
    fn call(&self, command: &str, args: Value)
        -> impl Future<Output = Result<Value, Value>> + Send;
}

fn received_of(value: &Value) -> Option<u64> {
    value.get("received").and_then(Value::as_u64)
}

fn bad_answer() -> Value {
    json!({ "code": "runtime_error", "message": "Your computer's answer could not be read." })
}

/// Send the file at `path` as `offer`, resuming where the computer stopped,
/// and return the computer's answer to [`FINISH`]. `progress` hears the
/// bytes the computer has after each part. An error is the computer's (or
/// the link's, `{ code: "offline" }`), as the phone shows it.
pub async fn send_file<P: Peer>(
    peer: &P,
    path: &Path,
    offer: &Offer,
    progress: impl Fn(u64),
) -> Result<Value, Value> {
    let offer_json = serde_json::to_value(offer).map_err(|_| bad_answer())?;
    let begun = peer.call(BEGIN, offer_json.clone()).await?;
    let transfer_id = begun
        .get("transferId")
        .and_then(Value::as_str)
        .ok_or_else(bad_answer)?
        .to_string();
    let mut offset = received_of(&begun).ok_or_else(bad_answer)?;
    let read_error = |e: std::io::Error| json!({ "code": "runtime_error", "message": format!("The file could not be read on this phone: {e}") });
    let mut file = File::open(path).map_err(read_error)?;
    let mut buffer = vec![0u8; CHUNK_BYTES];
    progress(offset);
    // Each offset may be refused once (the computer had more or less than the
    // phone thought); the phone then goes on from where the computer is.
    let mut resyncs = 0;
    while offset < offer.size {
        file.seek(SeekFrom::Start(offset)).map_err(read_error)?;
        let want =
            usize::try_from((offer.size - offset).min(CHUNK_BYTES as u64)).unwrap_or(CHUNK_BYTES);
        file.read_exact(&mut buffer[..want]).map_err(read_error)?;
        let part = json!({
            "transferId": transfer_id,
            "offset": offset,
            "data": BASE64.encode(&buffer[..want]),
        });
        match peer.call(CHUNK, part).await {
            Ok(answer) => {
                offset = received_of(&answer).ok_or_else(bad_answer)?;
                resyncs = 0;
            }
            Err(error)
                if error.get("code").and_then(Value::as_str) == Some("out_of_order")
                    && resyncs < 2 =>
            {
                resyncs += 1;
                offset = received_of(&error).ok_or_else(bad_answer)?;
            }
            Err(error) => return Err(error),
        }
        progress(offset);
    }
    peer.call(FINISH, json!({ "transferId": transfer_id }))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sealed::tests::temp_dir;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    const PHONE: &str = "phone-node";

    /// The computer's side, called directly; it stops answering (the link
    /// broke) after `limit` parts.
    struct Desktop {
        staging: Staging,
        limit: Option<usize>,
        parts: AtomicUsize,
        offsets: Mutex<Vec<u64>>,
        node: String,
    }

    impl Peer for Desktop {
        fn call(
            &self,
            command: &str,
            args: Value,
        ) -> impl Future<Output = Result<Value, Value>> + Send {
            let answer = (|| -> Result<Value, Value> {
                match command {
                    BEGIN => {
                        let offer: Offer =
                            serde_json::from_value(args).map_err(|_| json!("bad offer"))?;
                        let begun = self
                            .staging
                            .begin(&self.node, &offer, 1_000)
                            .map_err(|e| e.to_value())?;
                        Ok(serde_json::to_value(begun).unwrap())
                    }
                    CHUNK => {
                        if self
                            .limit
                            .is_some_and(|l| self.parts.load(Ordering::SeqCst) >= l)
                        {
                            return Err(
                                json!({ "code": "offline", "message": "Your phone is not connected to your computer." }),
                            );
                        }
                        self.parts.fetch_add(1, Ordering::SeqCst);
                        let part: Part =
                            serde_json::from_value(args).map_err(|_| json!("bad part"))?;
                        self.offsets.lock().unwrap().push(part.offset);
                        let done = self
                            .staging
                            .chunk(&self.node, &part)
                            .map_err(|e| e.to_value())?;
                        Ok(serde_json::to_value(done).unwrap())
                    }
                    FINISH => {
                        let id = args["transferId"].as_str().unwrap_or_default().to_string();
                        let arrived = self
                            .staging
                            .finish(&self.node, &id)
                            .map_err(|e| e.to_value())?;
                        let bytes = fs::read(&arrived.path).unwrap();
                        self.staging.forget(&id);
                        Ok(json!({ "name": arrived.offer.name, "bytes": bytes.len() }))
                    }
                    other => Err(json!(format!("unexpected {other}"))),
                }
            })();
            std::future::ready(answer)
        }
    }

    fn desktop(dir: &Path, limit: Option<usize>) -> Desktop {
        Desktop {
            staging: Staging::new(dir.join("staging")),
            limit,
            parts: AtomicUsize::new(0),
            offsets: Mutex::new(Vec::new()),
            node: PHONE.into(),
        }
    }

    fn a_file(dir: &Path, len: usize) -> (PathBuf, Offer) {
        let bytes: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        let path = dir.join("scan.pdf");
        fs::write(&path, &bytes).unwrap();
        let offer = Offer {
            name: "scan.pdf".into(),
            size: len as u64,
            sha256: sha256_hex(bytes.as_slice()).unwrap(),
            workspace_id: Some("w1".into()),
            purpose: Purpose::Library,
        };
        (path, offer)
    }

    #[tokio::test]
    async fn an_interrupted_transfer_resumes() {
        let dir = temp_dir("transfer-resume");
        let (path, offer) = a_file(&dir, CHUNK_BYTES * 3 + 1234);
        // The link breaks after two parts.
        let first = desktop(&dir, Some(2));
        let broke = send_file(&first, &path, &offer, |_| {}).await;
        assert_eq!(broke.unwrap_err()["code"], "offline");
        // Both sides start again (a new link, the same staging folder): the
        // phone goes on from the third part, not from the start.
        let again = desktop(&dir, None);
        let heard = Mutex::new(Vec::new());
        let finished = send_file(&again, &path, &offer, |n| heard.lock().unwrap().push(n))
            .await
            .unwrap();
        assert_eq!(finished["bytes"], json!(offer.size));
        assert_eq!(
            *again.offsets.lock().unwrap(),
            vec![2 * CHUNK_BYTES as u64, 3 * CHUNK_BYTES as u64],
            "only the parts the computer did not have"
        );
        assert_eq!(
            heard.lock().unwrap().first(),
            Some(&(2 * CHUNK_BYTES as u64))
        );
        assert_eq!(heard.lock().unwrap().last(), Some(&offer.size));
        assert!(
            fs::read_dir(dir.join("staging")).unwrap().next().is_none(),
            "nothing is left staged"
        );
    }

    #[test]
    fn a_part_at_the_wrong_offset_or_from_another_phone_is_refused() {
        let dir = temp_dir("transfer-rules");
        let staging = Staging::new(dir.join("staging"));
        let (_, offer) = a_file(&dir, 10);
        let begun = staging.begin(PHONE, &offer, 0).unwrap();
        assert_eq!(begun.received, 0);
        let part = |offset: u64, bytes: &[u8]| Part {
            transfer_id: begun.transfer_id.clone(),
            offset,
            data: BASE64.encode(bytes),
        };
        assert_eq!(
            staging.chunk(PHONE, &part(3, b"abc")),
            Err(TransferError::OutOfOrder { expected: 0 })
        );
        assert_eq!(
            staging.chunk("another-phone", &part(0, b"abc")),
            Err(TransferError::Unknown)
        );
        assert_eq!(
            staging.chunk(PHONE, &part(0, &[0; 11])),
            Err(TransferError::TooLong)
        );
        assert_eq!(
            staging.finish(PHONE, &begun.transfer_id).unwrap_err(),
            TransferError::Incomplete
        );
        // The wrong bytes, complete: refused and removed, so it starts again.
        staging.chunk(PHONE, &part(0, &[9; 10])).unwrap();
        assert_eq!(
            staging.finish(PHONE, &begun.transfer_id).unwrap_err(),
            TransferError::Changed
        );
        assert_eq!(staging.begin(PHONE, &offer, 0).unwrap().received, 0);
        // An id that is not one of ours never reaches the disk.
        assert!(matches!(
            staging.finish(PHONE, "../../etc/passwd"),
            Err(TransferError::Unknown)
        ));
        let mut bad = offer.clone();
        bad.sha256 = "zz".into();
        assert_eq!(staging.begin(PHONE, &bad, 0), Err(TransferError::Invalid));
    }

    #[test]
    fn a_phone_may_stage_only_so_much_and_old_parts_go() {
        let dir = temp_dir("transfer-limits");
        let staging = Staging::new(dir.join("staging"));
        let offer = |n: u64, size: u64| Offer {
            name: format!("f{n}.txt"),
            size,
            sha256: "a".repeat(64),
            workspace_id: None,
            purpose: Purpose::Library,
        };
        for n in 0..MAX_OPEN as u64 {
            staging.begin(PHONE, &offer(n, 10), 0).unwrap();
        }
        assert_eq!(
            staging.begin(PHONE, &offer(99, 10), 0),
            Err(TransferError::Busy)
        );
        // Another phone has its own room.
        staging.begin("other", &offer(99, 10), 0).unwrap();
        // A day later the abandoned parts are gone.
        staging.begin(PHONE, &offer(100, 10), STALE_MS + 1).unwrap();
        let big = Staging::new(dir.join("big"));
        assert_eq!(
            big.begin(PHONE, &offer(1, MAX_STAGED_BYTES + 1), 0),
            Err(TransferError::Full)
        );
    }
}
