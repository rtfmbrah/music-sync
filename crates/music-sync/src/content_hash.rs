//! Streaming cryptographic hashes for physical artifact evidence.

use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;

use sha2::{Digest, Sha256};
use thiserror::Error;

/// A successful content-hash observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentHash {
    /// SHA-256 digest of the exact file bytes.
    pub sha256: [u8; 32],
    /// Number of bytes streamed into the digest.
    pub bytes_hashed: u64,
}

/// Boundary for deriving exact physical-artifact evidence.
pub trait ContentHasher {
    /// Streams one file and hashes its exact bytes without modifying it.
    fn hash(&self, path: &Path) -> Result<ContentHash, ContentHashError>;
}

/// Streaming SHA-256 file hasher.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sha256FileHasher;

impl ContentHasher for Sha256FileHasher {
    fn hash(&self, path: &Path) -> Result<ContentHash, ContentHashError> {
        let file = File::open(path).map_err(|source| ContentHashError::Open {
            path: path.to_path_buf(),
            source,
        })?;
        let mut reader = BufReader::with_capacity(64 * 1024, file);
        let mut digest = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        let mut bytes_hashed = 0_u64;
        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|source| ContentHashError::Read {
                    path: path.to_path_buf(),
                    source,
                })?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
            bytes_hashed = bytes_hashed.saturating_add(read as u64);
        }
        Ok(ContentHash {
            sha256: digest.finalize().into(),
            bytes_hashed,
        })
    }
}

/// Failure to derive a content hash.
#[derive(Debug, Error)]
pub enum ContentHashError {
    /// The input file could not be opened.
    #[error("failed to open artifact {path}: {source}")]
    Open {
        /// Artifact path.
        path: std::path::PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },
    /// The input file could not be completely read.
    #[error("failed while hashing artifact {path}: {source}")]
    Read {
        /// Artifact path.
        path: std::path::PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_exact_bytes_with_known_sha256() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("fixture.bin");
        std::fs::write(&path, b"abc")?;

        let result = Sha256FileHasher.hash(&path)?;

        assert_eq!(result.bytes_hashed, 3);
        assert_eq!(
            result.sha256,
            [
                0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
                0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
                0xf2, 0x00, 0x15, 0xad,
            ]
        );
        Ok(())
    }
}
