use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use mr_crabs_terminal::{GridSize, NormalizedSnapshot, Terminal};
use serde::{Deserialize, Serialize};

pub const CORPUS_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OracleProvenance {
    pub engine: String,
    pub source_commit: String,
    pub fixture_format: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CorpusFile {
    pub schema_version: u32,
    pub oracle: OracleProvenance,
    pub cases: Vec<CorpusCase>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CorpusCase {
    pub name: String,
    pub size: GridSize,
    pub input_hex: String,
    pub chunking: Chunking,
    pub expected: NormalizedSnapshot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "strategy", rename_all = "snake_case")]
pub enum Chunking {
    AllSplits,
    SeededRandom {
        seed: u64,
        iterations: u16,
        max_chunk: u16,
    },
}

#[derive(Debug)]
pub struct OracleError(String);

impl OracleError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl Display for OracleError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for OracleError {}

pub fn run_corpus_dir(path: impl AsRef<Path>) -> Result<usize, OracleError> {
    let paths = corpus_paths(path.as_ref())?;
    let mut checked = 0;
    let mut names = BTreeSet::new();

    for path in paths {
        let corpus = read_corpus(&path)?;
        validate_corpus_header(&path, &corpus)?;
        for case in &corpus.cases {
            if !names.insert(case.name.clone()) {
                return Err(OracleError::new(format!(
                    "duplicate corpus case name {:?} in {}",
                    case.name,
                    path.display()
                )));
            }
            run_case(case)?;
            checked += 1;
        }
    }

    Ok(checked)
}

pub fn refresh_corpus_dir(
    oracle_executable: impl AsRef<Path>,
    corpus_dir: impl AsRef<Path>,
) -> Result<usize, OracleError> {
    let oracle_executable = oracle_executable.as_ref();
    let paths = corpus_paths(corpus_dir.as_ref())?;
    let mut refreshed = 0;

    for path in paths {
        let mut corpus = read_corpus(&path)?;
        validate_corpus_header(&path, &corpus)?;
        for case in &mut corpus.cases {
            let output = Command::new(oracle_executable)
                .arg("snapshot")
                .arg("--cols")
                .arg(case.size.cols.to_string())
                .arg("--rows")
                .arg(case.size.rows.to_string())
                .arg("--input-hex")
                .arg(&case.input_hex)
                .output()
                .map_err(|error| {
                    OracleError::new(format!(
                        "failed to execute Ghostty oracle {}: {error}",
                        oracle_executable.display()
                    ))
                })?;

            if !output.status.success() {
                return Err(OracleError::new(format!(
                    "Ghostty oracle failed for {:?} with {}: {}",
                    case.name,
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                )));
            }

            let snapshot: NormalizedSnapshot =
                serde_json::from_slice(&output.stdout).map_err(|error| {
                    OracleError::new(format!(
                        "Ghostty oracle returned invalid JSON for {:?}: {error}",
                        case.name
                    ))
                })?;
            if snapshot.size != case.size {
                return Err(OracleError::new(format!(
                    "Ghostty oracle returned size {:?} for {:?}, expected {:?}",
                    snapshot.size, case.name, case.size
                )));
            }
            case.expected = snapshot;
            refreshed += 1;
        }

        write_corpus_atomically(&path, &corpus)?;
    }

    Ok(refreshed)
}

fn corpus_paths(directory: &Path) -> Result<Vec<PathBuf>, OracleError> {
    let entries = fs::read_dir(directory).map_err(|error| {
        OracleError::new(format!(
            "failed to read corpus directory {}: {error}",
            directory.display()
        ))
    })?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            OracleError::new(format!(
                "failed to read entry in {}: {error}",
                directory.display()
            ))
        })?;
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            paths.push(path);
        }
    }
    paths.sort();
    if paths.is_empty() {
        return Err(OracleError::new(format!(
            "corpus directory {} contains no JSON fixtures",
            directory.display()
        )));
    }
    Ok(paths)
}

fn read_corpus(path: &Path) -> Result<CorpusFile, OracleError> {
    let bytes = fs::read(path)
        .map_err(|error| OracleError::new(format!("failed to read {}: {error}", path.display())))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| OracleError::new(format!("failed to parse {}: {error}", path.display())))
}

fn validate_corpus_header(path: &Path, corpus: &CorpusFile) -> Result<(), OracleError> {
    if corpus.schema_version != CORPUS_SCHEMA_VERSION {
        return Err(OracleError::new(format!(
            "unsupported corpus schema {} in {}; expected {}",
            corpus.schema_version,
            path.display(),
            CORPUS_SCHEMA_VERSION
        )));
    }
    if corpus.cases.is_empty() {
        return Err(OracleError::new(format!(
            "{} contains no corpus cases",
            path.display()
        )));
    }
    Ok(())
}

fn run_case(case: &CorpusCase) -> Result<(), OracleError> {
    if case.expected.size != case.size {
        return Err(OracleError::new(format!(
            "case {:?} has expectation size {:?}, but declares {:?}",
            case.name, case.expected.size, case.size
        )));
    }
    let input = decode_hex(&case.input_hex)
        .map_err(|error| OracleError::new(format!("case {:?}: {error}", case.name)))?;

    compare_snapshot(
        case,
        "whole stream",
        feed_chunks(case.size, &input, &[input.len()])?,
    )?;

    match case.chunking {
        Chunking::AllSplits => {
            for split in 0..=input.len() {
                let chunks = [&input[..split], &input[split..]];
                compare_snapshot(
                    case,
                    &format!("split at byte {split}"),
                    feed_slices(case.size, chunks)?,
                )?;
            }
        }
        Chunking::SeededRandom {
            seed,
            iterations,
            max_chunk,
        } => {
            if iterations == 0 {
                return Err(OracleError::new(format!(
                    "case {:?} must request at least one randomized iteration",
                    case.name
                )));
            }
            if max_chunk == 0 {
                return Err(OracleError::new(format!(
                    "case {:?} must use a nonzero maximum chunk size",
                    case.name
                )));
            }
            let mut state = if seed == 0 {
                0x9e37_79b9_7f4a_7c15
            } else {
                seed
            };
            for iteration in 0..iterations {
                let lengths =
                    randomized_chunk_lengths(input.len(), &mut state, usize::from(max_chunk));
                compare_snapshot(
                    case,
                    &format!("seeded chunking iteration {iteration}"),
                    feed_chunks(case.size, &input, &lengths)?,
                )?;
            }
        }
    }

    Ok(())
}

fn feed_chunks(
    size: GridSize,
    input: &[u8],
    lengths: &[usize],
) -> Result<NormalizedSnapshot, OracleError> {
    let mut terminal = Terminal::new(size)
        .map_err(|error| OracleError::new(format!("invalid terminal size: {error}")))?;
    let mut offset: usize = 0;
    for (index, &length) in lengths.iter().enumerate() {
        let end = offset
            .checked_add(length)
            .ok_or_else(|| OracleError::new("chunk overflow"))?;
        if end > input.len() {
            return Err(OracleError::new("chunk plan exceeds input length"));
        }
        terminal.feed(&input[offset..end]).map_err(|error| {
            OracleError::new(format!(
                "terminal feed failed at chunk {index} offset {offset} len {length} (input {} bytes): {error}",
                input.len()
            ))
        })?;
        offset = end;
    }
    if offset != input.len() {
        return Err(OracleError::new(
            "chunk plan does not consume the complete input",
        ));
    }
    Ok(terminal.snapshot())
}
fn feed_slices<'a>(
    size: GridSize,
    chunks: impl IntoIterator<Item = &'a [u8]>,
) -> Result<NormalizedSnapshot, OracleError> {
    let mut terminal = Terminal::new(size)
        .map_err(|error| OracleError::new(format!("invalid terminal size: {error}")))?;
    for (index, chunk) in chunks.into_iter().enumerate() {
        terminal.feed(chunk).map_err(|error| {
            OracleError::new(format!(
                "terminal feed failed at chunk {index} len {} bytes: {error}",
                chunk.len()
            ))
        })?;
    }
    Ok(terminal.snapshot())
}

fn compare_snapshot(
    case: &CorpusCase,
    chunking: &str,
    actual: NormalizedSnapshot,
) -> Result<(), OracleError> {
    if actual == case.expected {
        return Ok(());
    }

    let expected = serde_json::to_string_pretty(&case.expected)
        .unwrap_or_else(|error| format!("<failed to serialize expectation: {error}>"));
    let actual = serde_json::to_string_pretty(&actual)
        .unwrap_or_else(|error| format!("<failed to serialize result: {error}>"));
    Err(OracleError::new(format!(
        "corpus mismatch for {:?} ({chunking})\nexpected:\n{expected}\nactual:\n{actual}",
        case.name
    )))
}

fn randomized_chunk_lengths(total: usize, state: &mut u64, max_chunk: usize) -> Vec<usize> {
    let mut remaining: usize = total;
    let mut lengths: Vec<usize> = Vec::new();
    while remaining != 0 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        let upper: usize = remaining.min(max_chunk);
        let upper_u64: u64 = u64::try_from(upper).expect("chunk upper bound fits in u64");
        let remainder: u64 = *state % upper_u64;
        let remainder_usize: usize = usize::try_from(remainder).expect("remainder fits in usize");
        let length: usize = 1_usize
            .checked_add(remainder_usize)
            .expect("chunk length fits in usize");
        lengths.push(length);
        remaining = remaining.checked_sub(length).expect("remaining underflow");
    }
    lengths
}

fn decode_hex(input: &str) -> Result<Vec<u8>, OracleError> {
    if input.len() % 2 != 0 {
        return Err(OracleError::new(
            "input_hex must contain an even number of digits",
        ));
    }
    let mut bytes = Vec::with_capacity(input.len() / 2);
    for pair in input.as_bytes().chunks_exact(2) {
        let high = decode_nibble(pair[0])?;
        let low = decode_nibble(pair[1])?;
        bytes.push((high << 4) | low);
    }
    Ok(bytes)
}

fn decode_nibble(byte: u8) -> Result<u8, OracleError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(OracleError::new(format!(
            "input_hex contains non-hex byte {byte:?}"
        ))),
    }
}

fn write_corpus_atomically(path: &Path, corpus: &CorpusFile) -> Result<(), OracleError> {
    let mut bytes = serde_json::to_vec_pretty(corpus).map_err(|error| {
        OracleError::new(format!("failed to serialize {}: {error}", path.display()))
    })?;
    bytes.push(b'\n');
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, bytes).map_err(|error| {
        OracleError::new(format!("failed to write {}: {error}", temporary.display()))
    })?;
    fs::rename(&temporary, path).map_err(|error| {
        OracleError::new(format!(
            "failed to replace {} with {}: {error}",
            path.display(),
            temporary.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{decode_hex, randomized_chunk_lengths, run_corpus_dir};

    #[test]
    fn checked_in_ansi_dec_corpus_matches() {
        let corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../verification/corpus");
        let checked = run_corpus_dir(corpus).unwrap();
        assert!(checked >= 20);
    }

    #[test]
    fn seeded_chunking_consumes_every_byte() {
        let mut state = 0x5eed_u64;
        let lengths = randomized_chunk_lengths(4096, &mut state, 31);
        assert_eq!(lengths.iter().sum::<usize>(), 4096);
        assert!(lengths.iter().all(|length| (1..=31).contains(length)));
    }

    #[test]
    fn hex_input_is_strict() {
        assert_eq!(decode_hex("001bFF").unwrap(), [0x00, 0x1b, 0xff]);
        assert!(decode_hex("0").is_err());
        assert!(decode_hex("zz").is_err());
    }
}
