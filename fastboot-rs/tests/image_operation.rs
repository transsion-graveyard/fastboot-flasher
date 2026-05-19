//! Image operation tests for `fastboot-rs`.

use std::{collections::HashMap, fs::File, io::Write, path::Path};

use fastboot_rs::{
    current_slot, partition_with_slot, prepare_image, resolve_slot_suffix,
    sparse::{ChunkHeader, FileHeader, ParseError},
    write_transfer_payload, write_transfer_payload_with_progress, ImageKind, ImageTransfer,
    OperationSequence, OperationStep, SlotSelection,
};

#[test]
fn prepare_image_should_plan_single_raw_download_when_file_fits() {
    let temp = tempfile::tempdir().unwrap();
    let image_path = temp.path().join("boot.img");
    write_bytes(&image_path, &[0x5a; 512]);

    let prepared = prepare_image(&image_path, 4096).unwrap();

    assert_eq!(prepared.kind, ImageKind::Raw);
    assert_eq!(prepared.file_size, 512);
    assert_eq!(prepared.expanded_size, 512);
    assert_eq!(
        prepared.transfers,
        vec![ImageTransfer::Raw {
            range: fastboot_rs::RawRange {
                offset: 0,
                size: 512
            },
            download_size: 512,
        }]
    );
}

#[test]
fn prepare_image_should_split_large_raw_file_as_sparse_downloads() {
    let temp = tempfile::tempdir().unwrap();
    let image_path = temp.path().join("system.img");
    write_bytes(&image_path, &vec![0x33; 12 * 1024]);

    let prepared = prepare_image(&image_path, 8192).unwrap();

    assert_eq!(prepared.kind, ImageKind::Raw);
    assert!(prepared.transfers.len() > 1);
    assert!(prepared
        .transfers
        .iter()
        .all(|transfer| transfer.download_size() <= 8192));
}

#[test]
fn prepare_image_should_detect_and_split_android_sparse_images() {
    let temp = tempfile::tempdir().unwrap();
    let image_path = temp.path().join("vendor.img");
    write_sparse_raw_image(&image_path);

    let prepared = prepare_image(&image_path, 5000).unwrap();

    assert_eq!(prepared.kind, ImageKind::AndroidSparse);
    assert_eq!(prepared.expanded_size, 8192);
    assert!(prepared.transfer_count() > 1);
}

#[test]
fn operation_sequence_should_flash_once_per_prepared_transfer() {
    let temp = tempfile::tempdir().unwrap();
    let image_path = temp.path().join("system.img");
    write_bytes(&image_path, &vec![0x44; 12 * 1024]);
    let prepared = prepare_image(&image_path, 8192).unwrap();

    let sequence = OperationSequence::for_prepared_flash("system_a", &prepared);

    assert_eq!(sequence.steps.len(), prepared.transfer_count() * 2);
    assert_eq!(
        sequence.steps.last(),
        Some(&OperationStep::Flash {
            partition: "system_a".to_string()
        })
    );
}

#[test]
fn write_transfer_payload_should_emit_direct_raw_range() {
    let temp = tempfile::tempdir().unwrap();
    let image_path = temp.path().join("boot.img");
    write_bytes(&image_path, &[1, 2, 3, 4, 5]);
    let prepared = prepare_image(&image_path, 4096).unwrap();
    let mut out = Vec::new();

    write_transfer_payload(&image_path, &prepared.transfers[0], &mut out).unwrap();

    assert_eq!(out, vec![1, 2, 3, 4, 5]);
}

#[test]
fn write_transfer_payload_should_report_bytes_written() {
    let temp = tempfile::tempdir().unwrap();
    let image_path = temp.path().join("boot.img");
    write_bytes(&image_path, &[1, 2, 3, 4, 5]);
    let prepared = prepare_image(&image_path, 4096).unwrap();
    let mut out = Vec::new();
    let mut transferred = 0;

    write_transfer_payload_with_progress(&image_path, &prepared.transfers[0], &mut out, |bytes| {
        transferred += bytes;
    })
    .unwrap();

    assert_eq!(transferred, 5);
}

#[test]
fn write_transfer_payload_should_emit_sparse_split_payload() {
    let temp = tempfile::tempdir().unwrap();
    let image_path = temp.path().join("system.img");
    write_bytes(&image_path, &vec![0x33; 12 * 1024]);
    let prepared = prepare_image(&image_path, 8192).unwrap();
    let sparse = prepared
        .transfers
        .iter()
        .find(|transfer| matches!(transfer, ImageTransfer::Sparse { .. }))
        .unwrap();
    let mut out = Vec::new();

    write_transfer_payload(&image_path, sparse, &mut out).unwrap();

    assert_eq!(out.len(), sparse.download_size() as usize);
}

#[test]
fn chunk_out_size_should_error_on_overflow() {
    let header = FileHeader {
        block_size: u32::MAX,
        blocks: 0,
        chunks: 0,
        checksum: 0,
    };
    let chunk = ChunkHeader {
        chunk_type: fastboot_rs::sparse::ChunkType::Raw,
        chunk_size: u32::MAX,
        total_size: 0,
    };
    let out_size = chunk.out_size(&header);

    if usize::BITS == 32 {
        assert_eq!(out_size.unwrap_err(), ParseError::ChunkOutputSizeOverflow);
    } else {
        assert_eq!(out_size.unwrap(), u64::from(u32::MAX).pow(2) as usize);
    }
}

#[test]
fn slot_helpers_should_resolve_active_and_inactive_suffixes() {
    let vars = HashMap::from([("current-slot".to_string(), "_a".to_string())]);

    assert_eq!(current_slot(&vars).unwrap(), "a");
    assert_eq!(
        resolve_slot_suffix(SlotSelection::Inactive, &vars).unwrap(),
        "b"
    );
    assert_eq!(partition_with_slot("boot", "b"), "boot_b");
}

fn write_bytes(path: &Path, bytes: &[u8]) {
    let mut file = File::create(path).unwrap();
    file.write_all(bytes).unwrap();
}

fn write_sparse_raw_image(path: &Path) {
    let header = FileHeader {
        block_size: 4096,
        blocks: 2,
        chunks: 1,
        checksum: 0,
    };
    let chunk = ChunkHeader::new_raw(2, 4096);
    let mut file = File::create(path).unwrap();
    file.write_all(&header.to_bytes()).unwrap();
    file.write_all(&chunk.to_bytes()).unwrap();
    file.write_all(&vec![0x22; chunk.data_size()]).unwrap();
}
