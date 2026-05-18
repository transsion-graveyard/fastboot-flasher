//! High-level fastboot execution helpers.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use thiserror::Error;

use crate::{
    image::{ImagePayloadError, ImageTransfer, PreparedImage},
    transport::nusb::{DataDownload, DownloadError, NusbFastBoot, NusbFastBootError},
};

const STREAM_CHUNK_SIZE: usize = 64 * 1024;

/// Progress events emitted while flashing a prepared image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashProgress {
    /// A transfer payload is about to be downloaded.
    DownloadStarted {
        transfer_index: usize,
        transfer_count: usize,
        bytes: u32,
    },
    /// Download payload bytes have been queued.
    DownloadFinished {
        transfer_index: usize,
        transfer_count: usize,
        bytes: u32,
    },
    /// Download payload bytes have been materialized locally.
    DownloadBytes {
        transfer_index: usize,
        transfer_count: usize,
        bytes: u64,
    },
    /// The device flash command is about to run.
    FlashStarted {
        transfer_index: usize,
        transfer_count: usize,
    },
    /// The device flash command completed.
    FlashFinished {
        transfer_index: usize,
        transfer_count: usize,
    },
}

/// Errors while executing prepared fastboot operations.
#[derive(Debug, Error)]
pub enum FastbootExecutionError {
    /// Fastboot transport failed.
    #[error(transparent)]
    Fastboot(#[from] NusbFastBootError),
    /// Download streaming failed.
    #[error(transparent)]
    Download(#[from] DownloadError),
    /// Transfer payload materialization failed.
    #[error(transparent)]
    Payload(#[from] ImagePayloadError),
}

/// Download and flash all transfers for one prepared partition image.
pub async fn flash_prepared_image(
    fastboot: &mut NusbFastBoot,
    partition: &str,
    image: &PreparedImage,
    mut progress: impl FnMut(FlashProgress),
) -> Result<(), FastbootExecutionError> {
    let transfer_count = image.transfers.len();
    for (index, transfer) in image.transfers.iter().enumerate() {
        let transfer_index = index + 1;
        let bytes = transfer.download_size();
        progress(FlashProgress::DownloadStarted {
            transfer_index,
            transfer_count,
            bytes,
        });
        let mut download = fastboot.download(bytes).await?;
        stream_transfer_payload(&mut download, &image.path, transfer, |bytes| {
            progress(FlashProgress::DownloadBytes {
                transfer_index,
                transfer_count,
                bytes,
            });
        })
        .await?;
        download.finish().await?;
        progress(FlashProgress::DownloadFinished {
            transfer_index,
            transfer_count,
            bytes,
        });
        progress(FlashProgress::FlashStarted {
            transfer_index,
            transfer_count,
        });
        fastboot.flash(partition).await?;
        progress(FlashProgress::FlashFinished {
            transfer_index,
            transfer_count,
        });
    }
    Ok(())
}

async fn stream_transfer_payload(
    download: &mut DataDownload<'_>,
    image_path: &Path,
    transfer: &ImageTransfer,
    mut progress: impl FnMut(u64),
) -> Result<(), FastbootExecutionError> {
    let mut file = File::open(image_path).map_err(ImagePayloadError::from)?;
    match transfer {
        ImageTransfer::Raw { range, .. } => {
            file.seek(SeekFrom::Start(range.offset))
                .map_err(ImagePayloadError::from)?;
            copy_exact_to_download(&mut file, download, range.size, &mut progress).await?;
        }
        ImageTransfer::Sparse { split, .. } => {
            extend_download(download, &split.header.to_bytes(), &mut progress).await?;
            for chunk in &split.chunks {
                extend_download(download, &chunk.header.to_bytes(), &mut progress).await?;
                if chunk.size == 0 {
                    continue;
                }
                file.seek(SeekFrom::Start(chunk.offset as u64))
                    .map_err(ImagePayloadError::from)?;
                copy_exact_padded_to_download(&mut file, download, chunk.size, &mut progress)
                    .await?;
            }
        }
    }
    Ok(())
}

async fn copy_exact_to_download(
    file: &mut File,
    download: &mut DataDownload<'_>,
    size: u64,
    progress: &mut impl FnMut(u64),
) -> Result<(), FastbootExecutionError> {
    let mut left = size;
    let mut buf = [0u8; STREAM_CHUNK_SIZE];
    while left > 0 {
        let chunk_len = usize::try_from(left.min(buf.len() as u64))
            .map_err(|_| ImagePayloadError::SizeTooLarge(left))?;
        file.read_exact(&mut buf[..chunk_len])
            .map_err(ImagePayloadError::from)?;
        extend_download(download, &buf[..chunk_len], progress).await?;
        left -= chunk_len as u64;
    }
    Ok(())
}

async fn copy_exact_padded_to_download(
    file: &mut File,
    download: &mut DataDownload<'_>,
    size: usize,
    progress: &mut impl FnMut(u64),
) -> Result<(), FastbootExecutionError> {
    let mut left = size;
    let mut buf = [0u8; STREAM_CHUNK_SIZE];
    while left > 0 {
        let chunk_len = left.min(buf.len());
        let mut filled = 0;
        while filled < chunk_len {
            match file.read(&mut buf[filled..chunk_len]) {
                Ok(0) => {
                    buf[filled..chunk_len].fill(0);
                    filled = chunk_len;
                }
                Ok(read) => filled += read,
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                Err(err) => return Err(ImagePayloadError::from(err).into()),
            }
        }
        extend_download(download, &buf[..chunk_len], progress).await?;
        left -= chunk_len;
    }
    Ok(())
}

async fn extend_download(
    download: &mut DataDownload<'_>,
    bytes: &[u8],
    progress: &mut impl FnMut(u64),
) -> Result<(), FastbootExecutionError> {
    download.extend_from_slice(bytes).await?;
    progress(bytes.len() as u64);
    Ok(())
}
