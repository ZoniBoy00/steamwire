#![no_main]

use libfuzzer_sys::fuzz_target;
use steamwire::cdn::{chunk, ChunkData};
use steamwire::util::compression::{vzip_decompress, vzip_delta_decompress, vzstd_decompress};
use steamwire::util::lzma;

fuzz_target!(|data: &[u8]| {
    let mut out = vec![0u8; 1 << 16];
    let _ = vzip_decompress(data, &mut out, true);
    let _ = vzstd_decompress(data, &mut out, true);
    let _ = vzip_delta_decompress(data, &[1u8; 4096]);
    if data.len() >= 5 {
        let _ = lzma::decode(&data[..5], &data[5..], &mut out);
    }
    let info = ChunkData { uncompressed_length: 1 << 16, ..Default::default() };
    let _ = chunk::process(&info, data, &mut out, &[3; 32]);
});
