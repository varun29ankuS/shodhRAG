//! The library both sides of the upgrade test agree on: the files indexed,
//! the deterministic embedder, the memories and the manifest format.
//!
//! Shared by the writer (`write_library.rs`, built and run against the
//! `v0.0.10` tag by CI's "Upgrade from 0.0.10" job) and the reader
//! (`src/upgrade_tests.rs`, this build). It uses only `std` and `serde`, which
//! both versions of the crate depend on, so the same file compiles on both.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The production embedding dimension (multilingual-e5-base).
pub const DIMENSION: usize = 768;

/// The environment variable naming the fixture folder (the writer's output).
pub const FIXTURE_ENV: &str = "SHODH_UPGRADE_FIXTURE";

/// The manifest file in the fixture folder.
pub const MANIFEST_FILE: &str = "manifest.json";
/// The app data folder in the fixture folder (what 0.0.10 kept under its
/// data directory: `lance_data`, `tantivy_index`, `shodh.db`).
pub const DATA_DIR: &str = "data";
/// The indexed files in the fixture folder.
pub const FILES_DIR: &str = "files";

/// The Library folder source: its id (the `space_id` of its files), its
/// folder under [`FILES_DIR`], and where folder sync keeps its manifests in
/// the data folder (the app's `folder_sync`).
pub const FOLDER_SOURCE: &str = "field-notes";
pub const FOLDER: &str = "notes";
pub const FOLDER_MANIFESTS: &str = "folder_sync";
/// The source (`space_id`) of the files indexed one by one.
pub const FILE_SOURCE: &str = "library-files";

/// The pseudo-source of the note added without a file.
pub const NOTE_SOURCE: &str = "note://upgrade-fixture";
/// A word only the note holds.
pub const NOTE_TOKEN: &str = "pellucidar";
/// The note's text.
pub const NOTE_TEXT: &str = "Pellucidar is the inner world of the expedition notes. The crew \
     mapped its rivers by lantern light and kept the soundings in a ledger.";

/// Words in the bulk volumes: enough of them for the library to pass the
/// 1,000 chunks from which 0.0.10 built its L2 IVF-PQ vector index.
const BULK_VOLUMES: usize = 30;
const BULK_WORDS_PER_VOLUME: usize = 9_000;

/// A file of the library.
#[derive(Debug, Clone)]
pub struct LibraryFile {
    /// Path under [`FILES_DIR`], with `/` separators.
    pub path: String,
    /// The library source (`space_id`) it belongs to.
    pub space: &'static str,
    /// A word only this file holds, searched for after the upgrade.
    pub token: String,
    pub text: String,
    /// Indexed after 0.0.10 built its vector index and removed a file: rows
    /// its index does not cover.
    pub late: bool,
    /// Removed by 0.0.10 after its index was built.
    pub removed: bool,
}

/// What [`LibraryFile::kind`] says 0.0.11 does with the file's chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Markdown: structure chunks, raised to the new chunker version in place.
    Structure,
    /// Plain text and code: window chunks, read and chunked again.
    Window,
    /// A file that may hold secrets: 0.0.11 removes it from the index.
    Secret,
}

impl LibraryFile {
    /// In the folder source ([`FOLDER`]): indexed by folder sync.
    pub fn in_folder(&self) -> bool {
        self.path.starts_with(&format!("{FOLDER}/"))
    }

    pub fn kind(&self) -> Kind {
        if self.path.ends_with(".md") {
            Kind::Structure
        } else if self.path.ends_with(".env") {
            Kind::Secret
        } else {
            Kind::Window
        }
    }
}

fn file(path: &str, space: &'static str, token: &str, text: String) -> LibraryFile {
    LibraryFile {
        path: path.to_string(),
        space,
        token: token.to_string(),
        text,
        late: false,
        removed: false,
    }
}

/// Every file of the library, in the order the writer indexes them.
pub fn files() -> Vec<LibraryFile> {
    let mut files = vec![
        file(
            "notes/zephyrine-tides.md",
            FOLDER_SOURCE,
            "zephyrine",
            "# Zephyrine tides\n\n\
             The zephyrine tide arrives twice a lunar month at the northern estuary.\n\n\
             ## Measurements\n\n\
             Gauges on the old pier record the height every ten minutes. The zephyrine \
             peak is the highest reading between the two slack waters.\n\n\
             ## Notes\n\n\
             Boats stay moored while the tide turns; the channel shifts after each storm.\n"
                .to_string(),
        ),
        file(
            "notes/quillwort-ferns.md",
            FOLDER_SOURCE,
            "quillwort",
            "# Quillwort survey\n\n\
             Quillwort grows submerged along the shallow margins of the upland lakes.\n\n\
             ## Method\n\n\
             Each transect is walked at low water and every quillwort rosette within a \
             metre of the line is counted.\n\n\
             ## Results\n\n\
             The western lake held three times as many rosettes as the eastern one.\n"
                .to_string(),
        ),
        file(
            "notes/tide-table.txt",
            FOLDER_SOURCE,
            "marigraph",
            "Tide table for the northern estuary, read from the marigraph on the old \
             pier. High water at twelve past six and twenty to seven; low water at five \
             past midnight. The marigraph drum is changed every Monday."
                .to_string(),
        ),
        file(
            "code/ledger.rs",
            FILE_SOURCE,
            "grimthorpe",
            "/// Totals the grimthorpe ledger by account.\n\
             pub fn totals(entries: &[(String, i64)]) -> std::collections::BTreeMap<String, i64> {\n\
             \x20   let mut grimthorpe = std::collections::BTreeMap::new();\n\
             \x20   for (account, amount) in entries {\n\
             \x20       *grimthorpe.entry(account.clone()).or_insert(0) += amount;\n\
             \x20   }\n\
             \x20   grimthorpe\n\
             }\n"
                .to_string(),
        ),
        file(
            "code/route.ts",
            FILE_SOURCE,
            "wombatize",
            "// Turns a list of stops into a route, wombatize style: nearest stop first.\n\
             export function wombatize(stops: { x: number; y: number }[]): number[] {\n\
             \x20 const order: number[] = [];\n\
             \x20 const left = stops.map((_, i) => i);\n\
             \x20 let at = { x: 0, y: 0 };\n\
             \x20 while (left.length > 0) {\n\
             \x20   left.sort((a, b) => dist(stops[a], at) - dist(stops[b], at));\n\
             \x20   const next = left.shift()!;\n\
             \x20   order.push(next);\n\
             \x20   at = stops[next];\n\
             \x20 }\n\
             \x20 return order;\n\
             }\n\
             function dist(a: { x: number; y: number }, b: { x: number; y: number }): number {\n\
             \x20 return Math.hypot(a.x - b.x, a.y - b.y);\n\
             }\n"
                .to_string(),
        ),
        file(
            "text/harbor-log.txt",
            FILE_SOURCE,
            "kestrelwick",
            "Harbor log, spring. The kestrelwick ferry ran on time all week. Fog on \
             Thursday delayed the morning crossing by forty minutes; the afternoon \
             crossings were full. The harbour master asked for a second kestrelwick \
             sailing on market days."
                .to_string(),
        ),
        file(
            "drafts/obsolete-plan.txt",
            FILE_SOURCE,
            "brontide",
            "An old plan for the brontide recordings, replaced by the new schedule. \
             The brontide microphones were to be set on the ridge."
                .to_string(),
        ),
        file(
            "config/.env",
            FILE_SOURCE,
            "QUODDYWHISTLE",
            "QUODDYWHISTLE=not-a-real-token-0000\n".to_string(),
        ),
        file(
            "text/late-addition.txt",
            FILE_SOURCE,
            "sassafrine",
            "Added after the index was built: the sassafrine orchard flowers in late \
             spring and its fruit is picked in the first week of autumn."
                .to_string(),
        ),
    ];
    for f in &mut files {
        f.removed = f.path == "drafts/obsolete-plan.txt";
        f.late = f.path == "text/late-addition.txt";
    }
    let late = files.pop().expect("the late file is last");
    for volume in 0..BULK_VOLUMES {
        let token = volume_token(volume);
        files.push(file(
            &format!("archive/volume-{volume:02}.txt"),
            "archive",
            &token,
            bulk_text(volume, &token),
        ));
    }
    files.push(late);
    files
}

/// The word only bulk volume `volume` holds.
pub fn volume_token(volume: usize) -> String {
    let letters = ["q", "v", "z", "x", "j", "k"];
    format!(
        "archivum{}{}{}",
        letters[volume % 6],
        letters[(volume / 6) % 6],
        letters[(volume / 36) % 6]
    )
}

/// splitmix64: a fixed generator, so the volumes are the same on every run.
fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Pseudo-words built from syllables: a vocabulary large enough that chunks
/// differ, small enough that they share words like real prose.
fn bulk_text(volume: usize, token: &str) -> String {
    const SYLLABLES: [&str; 24] = [
        "ba", "ce", "di", "fo", "gu", "ha", "je", "ki", "lo", "mu", "na", "pe", "ri", "so", "tu",
        "va", "we", "xi", "yo", "za", "ar", "en", "il", "or",
    ];
    let mut state = 0x5EED_0010_0000_0000_u64 + volume as u64;
    let mut out = String::with_capacity(BULK_WORDS_PER_VOLUME * 8);
    for i in 0..BULK_WORDS_PER_VOLUME {
        if i % 600 == 0 {
            // The volume's own word, once in every few chunks.
            out.push_str(token);
            out.push(' ');
        }
        let r = next(&mut state);
        let syllables = 2 + (r % 3) as usize;
        for s in 0..syllables {
            out.push_str(SYLLABLES[((r >> (8 + 5 * s)) % 24) as usize]);
        }
        out.push(if i % 14 == 13 { '.' } else { ' ' });
        if i % 14 == 13 {
            out.push(' ');
        }
    }
    out
}

/// FNV-1a: a hash fixed by its definition, unlike std's `DefaultHasher`,
/// whose output may change between Rust releases.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The embedder of both sides: each lower-cased word hashed to one of
/// [`DIMENSION`] dimensions with a sign, then L2-normalised. Texts that share
/// words are similar; a word only one file holds finds that file.
pub fn embed(text: &str) -> Vec<f32> {
    let mut v = vec![0.0f32; DIMENSION];
    for word in text.split(|c: char| !c.is_alphanumeric()) {
        if word.is_empty() {
            continue;
        }
        let hash = fnv1a(word.to_lowercase().as_bytes());
        let index = (hash % DIMENSION as u64) as usize;
        let sign = if hash >> 63 == 0 { 1.0 } else { -1.0 };
        v[index] += sign;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 {
        v[DIMENSION - 1] = 1.0;
    } else {
        v.iter_mut().for_each(|x| *x /= norm);
    }
    v
}

/// The memories the writer saves (as notes), each with a word only it holds.
pub fn memories() -> Vec<(&'static str, &'static str, Option<&'static str>)> {
    vec![
        (
            "galingale",
            "I prefer galingale in the curry, never ginger.",
            None,
        ),
        (
            "tamarisk",
            "The tamarisk survey report is due on the first Monday of each quarter.",
            None,
        ),
        (
            "obsidianne",
            "In the field notes, cite gauges by their obsidianne serial numbers.",
            Some("field-notes"),
        ),
    ]
}

/// A chunk row of the documents table, every column as stored. The vector is
/// kept as the SHA-256 of its little-endian `f32` bytes: any changed bit of
/// any dimension changes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkRow {
    pub id: String,
    pub doc_id: String,
    pub chunk_index: u32,
    pub text: String,
    pub title: String,
    pub source: String,
    pub heading: String,
    pub vector_sha256: String,
    pub space_id: String,
    pub metadata_json: String,
    pub citation_json: String,
    pub created_at: i64,
}

/// A file as 0.0.10 stored it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredFile {
    /// [`LibraryFile::path`].
    pub path: String,
    /// The `source` column of its chunks (0.0.10's normalised path).
    pub source: String,
    pub token: String,
    pub removed: bool,
}

/// What a search for a word returned in 0.0.10: the sources, best first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchRecord {
    pub query: String,
    pub sources: Vec<String>,
}

/// A memory as 0.0.10 listed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRow {
    pub id: String,
    pub token: String,
    pub class: String,
    pub text: String,
    /// `global`, or the workspace id.
    pub scope: String,
    pub source: String,
    pub created_at: String,
}

/// Everything 0.0.10 read back from the library it wrote: what the reader
/// compares the upgraded library with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// The app version that wrote it (`CARGO_PKG_VERSION` of shodh-rag).
    pub written_by: String,
    /// The fixture folder when it was written: sources are absolute paths
    /// under it, so the reader must find it at the same place.
    pub fixture_dir: String,
    pub files: Vec<StoredFile>,
    /// Every row of the documents table, by id.
    pub chunks: BTreeMap<String, ChunkRow>,
    /// The documents table's indices: (name, type, columns).
    pub indices: Vec<(String, String, Vec<String>)>,
    /// The vector index's rows: (indexed, not indexed).
    pub vector_index_rows: (usize, usize),
    /// Documents in the keyword index (Tantivy).
    pub keyword_documents: usize,
    pub searches: Vec<SearchRecord>,
    pub memories: Vec<MemoryRow>,
    /// Rows of the memory statements table.
    pub statement_rows: usize,
}

/// The SHA-256 of `vector`'s little-endian bytes, as lower-case hex.
pub fn vector_digest<D: FnOnce(&[u8]) -> Vec<u8>>(vector: &[f32], sha256: D) -> String {
    let mut bytes = Vec::with_capacity(vector.len() * 4);
    for x in vector {
        bytes.extend_from_slice(&x.to_le_bytes());
    }
    sha256(&bytes).iter().map(|b| format!("{b:02x}")).collect()
}
