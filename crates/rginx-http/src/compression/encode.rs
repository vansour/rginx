use std::io::Write;

use brotli::CompressorWriter;
use flate2::Compression;
use flate2::write::GzEncoder;

use super::ContentCoding;

pub(super) fn compress_bytes(coding: ContentCoding, bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    match coding {
        ContentCoding::Brotli => brotli_bytes(bytes),
        ContentCoding::Gzip => gzip_bytes(bytes),
    }
}

fn brotli_bytes(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut compressed = Vec::with_capacity(bytes.len() / 2);
    {
        let mut encoder = CompressorWriter::new(&mut compressed, 4096, 5, 22);
        encoder.write_all(bytes)?;
        encoder.flush()?;
    }
    Ok(compressed)
}

fn gzip_bytes(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::with_capacity(bytes.len() / 2), Compression::default());
    encoder.write_all(bytes)?;
    encoder.finish()
}
