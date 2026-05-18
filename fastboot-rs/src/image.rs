use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::sparse::{
    split::{split_image, split_raw, Split, SplitError},
    ChunkHeader, FileHeader, FileHeaderBytes, CHUNK_HEADER_BYTES_LEN, FILE_HEADER_BYTES_LEN,
};

/// The image encoding detected from the file header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    /// A plain raw partition image.
    Raw,
    /// Android sparse image format.
    AndroidSparse,
}

/// A contiguous range read from the source image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawRange {
    /// Source file offset in bytes.
    pub offset: u64,
    /// Source byte count.
    pub size: u64,
}

/// One fastboot download payload planned for an image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageTransfer {
    /// Direct raw download of a source file range.
    Raw {
        /// Source range to stream.
        range: RawRange,
        /// Number of bytes passed to `download:`.
        download_size: u32,
    },
    /// Android sparse payload assembled from a split description.
    Sparse {
        /// Sparse split metadata.
        split: Split,
        /// Number of bytes passed to `download:`.
        download_size: u32,
    },
}

/// Device-free image metadata and download plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedImage {
    /// Source file path.
    pub path: PathBuf,
    /// Source file size in bytes.
    pub file_size: u64,
    /// Detected source image kind.
    pub kind: ImageKind,
    /// Expanded partition bytes for sparse images, or raw file size for raw images.
    pub expanded_size: u64,
    /// Fastboot download payloads needed to flash the image.
    pub transfers: Vec<ImageTransfer>,
}

impl PreparedImage {
    /// Number of download/flash rounds needed for this image.
    pub fn transfer_count(&self) -> usize {
        self.transfers.len()
    }
}

/// Image preparation failures.
#[derive(Debug, Error)]
pub enum ImagePreparationError {
    /// File I/O failed.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// Sparse image parsing failed.
    #[error("failed to parse Android sparse image: {0}")]
    SparseParse(#[from] crate::sparse::ParseError),
    /// Image splitting failed.
    #[error("failed to split image for max download size: {0}")]
    Split(#[from] SplitError),
    /// A raw direct download cannot be represented by the fastboot protocol.
    #[error("image size {size} exceeds fastboot download limit {limit}")]
    SizeTooLarge { size: u64, limit: u64 },
}

/// Errors while materializing a planned image transfer.
#[derive(Debug, Error)]
pub enum ImagePayloadError {
    /// File I/O failed.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// The transfer length cannot be represented on this platform.
    #[error("transfer size {0} is too large for this platform")]
    SizeTooLarge(u64),
}

/// Inspect and split an image for a device `max-download-size`.
pub fn prepare_image(
    path: impl AsRef<Path>,
    max_download_size: u32,
) -> Result<PreparedImage, ImagePreparationError> {
    let path = path.as_ref();
    let mut file = File::open(path)?;
    let file_size = file.metadata()?.len();
    let mut header_bytes: FileHeaderBytes = [0; FILE_HEADER_BYTES_LEN];
    let header_read = file.read(&mut header_bytes)?;

    if header_read == FILE_HEADER_BYTES_LEN {
        match FileHeader::from_bytes(&header_bytes) {
            Ok(header) => {
                return prepare_sparse(path, file, file_size, header, max_download_size);
            }
            Err(crate::sparse::ParseError::UnknownMagic) => {}
            Err(err) => return Err(err.into()),
        }
    }

    prepare_raw(path, file_size, max_download_size)
}

/// Write one planned fastboot download payload to `writer`.
pub fn write_transfer_payload(
    image_path: impl AsRef<Path>,
    transfer: &ImageTransfer,
    writer: &mut impl Write,
) -> Result<(), ImagePayloadError> {
    write_transfer_payload_with_progress(image_path, transfer, writer, |_| {})
}

/// Write one planned fastboot download payload and report bytes written.
pub fn write_transfer_payload_with_progress(
    image_path: impl AsRef<Path>,
    transfer: &ImageTransfer,
    writer: &mut impl Write,
    mut progress: impl FnMut(u64),
) -> Result<(), ImagePayloadError> {
    let mut file = File::open(image_path)?;
    match transfer {
        ImageTransfer::Raw { range, .. } => {
            file.seek(SeekFrom::Start(range.offset))?;
            copy_exact(&mut file, writer, range.size, &mut progress)?;
        }
        ImageTransfer::Sparse { split, .. } => {
            writer.write_all(&split.header.to_bytes())?;
            progress(FILE_HEADER_BYTES_LEN as u64);
            for chunk in &split.chunks {
                writer.write_all(&chunk.header.to_bytes())?;
                progress(CHUNK_HEADER_BYTES_LEN as u64);
                if chunk.size == 0 {
                    continue;
                }
                file.seek(SeekFrom::Start(chunk.offset as u64))?;
                copy_exact_padded(&mut file, writer, chunk.size, &mut progress)?;
            }
        }
    }
    Ok(())
}

fn prepare_sparse(
    path: &Path,
    mut file: File,
    file_size: u64,
    header: FileHeader,
    max_download_size: u32,
) -> Result<PreparedImage, ImagePreparationError> {
    let mut chunks = Vec::with_capacity(header.chunks as usize);
    for _ in 0..header.chunks {
        let mut chunk_bytes = [0; CHUNK_HEADER_BYTES_LEN];
        file.read_exact(&mut chunk_bytes)?;
        let chunk = ChunkHeader::from_bytes(&chunk_bytes)?;
        file.seek(SeekFrom::Current(chunk.data_size() as i64))?;
        chunks.push(chunk);
    }

    let transfers = split_image(&header, &chunks, max_download_size)?
        .into_iter()
        .map(|split| {
            let download_size = u32::try_from(split.sparse_size()).map_err(|_| {
                ImagePreparationError::SizeTooLarge {
                    size: split.sparse_size() as u64,
                    limit: u32::MAX as u64,
                }
            })?;
            Ok(ImageTransfer::Sparse {
                split,
                download_size,
            })
        })
        .collect::<Result<Vec<_>, ImagePreparationError>>()?;

    Ok(PreparedImage {
        path: path.to_path_buf(),
        file_size,
        kind: ImageKind::AndroidSparse,
        expanded_size: header.total_size() as u64,
        transfers,
    })
}

fn prepare_raw(
    path: &Path,
    file_size: u64,
    max_download_size: u32,
) -> Result<PreparedImage, ImagePreparationError> {
    let transfers = if file_size <= u64::from(max_download_size) {
        let download_size =
            u32::try_from(file_size).map_err(|_| ImagePreparationError::SizeTooLarge {
                size: file_size,
                limit: u32::MAX as u64,
            })?;
        vec![ImageTransfer::Raw {
            range: RawRange {
                offset: 0,
                size: file_size,
            },
            download_size,
        }]
    } else {
        let raw_size =
            usize::try_from(file_size).map_err(|_| ImagePreparationError::SizeTooLarge {
                size: file_size,
                limit: usize::MAX as u64,
            })?;
        split_raw(raw_size, max_download_size)?
            .into_iter()
            .map(|split| {
                let download_size = u32::try_from(split.sparse_size()).map_err(|_| {
                    ImagePreparationError::SizeTooLarge {
                        size: split.sparse_size() as u64,
                        limit: u32::MAX as u64,
                    }
                })?;
                Ok(ImageTransfer::Sparse {
                    split,
                    download_size,
                })
            })
            .collect::<Result<Vec<_>, ImagePreparationError>>()?
    };

    Ok(PreparedImage {
        path: path.to_path_buf(),
        file_size,
        kind: ImageKind::Raw,
        expanded_size: file_size,
        transfers,
    })
}

fn copy_exact(
    reader: &mut impl Read,
    writer: &mut impl Write,
    size: u64,
    progress: &mut impl FnMut(u64),
) -> Result<(), ImagePayloadError> {
    let mut limited = reader.take(size);
    let copied = std::io::copy(&mut limited, writer)?;
    if copied == size {
        progress(copied);
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!("expected {size} bytes, copied {copied}"),
        )
        .into())
    }
}

fn copy_exact_padded(
    reader: &mut impl Read,
    writer: &mut impl Write,
    size: usize,
    progress: &mut impl FnMut(u64),
) -> Result<(), ImagePayloadError> {
    let mut left = size;
    let mut buf = [0u8; 64 * 1024];
    while left > 0 {
        let chunk_len = left.min(buf.len());
        let mut filled = 0;
        while filled < chunk_len {
            match reader.read(&mut buf[filled..chunk_len]) {
                Ok(0) => {
                    buf[filled..chunk_len].fill(0);
                    filled = chunk_len;
                }
                Ok(read) => filled += read,
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                Err(err) => return Err(err.into()),
            }
        }
        writer.write_all(&buf[..chunk_len])?;
        progress(chunk_len as u64);
        left -= chunk_len;
    }
    Ok(())
}

/// Source image reference for operation planning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageSource {
    /// Path to the image file.
    pub path: PathBuf,
    /// Optional known partition size in bytes.
    pub partition_size: Option<u64>,
}

impl ImageSource {
    /// Create an image source from a path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            partition_size: None,
        }
    }

    /// Attach the target partition size.
    pub fn with_partition_size(mut self, partition_size: u64) -> Self {
        self.partition_size = Some(partition_size);
        self
    }
}
