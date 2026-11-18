#![no_main]

use libfuzzer_sys::fuzz_target;
use steamwire::cdn::DepotManifest;
use steamwire::util::compression::zip_decompress;

fuzz_target!(|data: &[u8]| {
    if let Ok(mut manifest) = DepotManifest::deserialize(data) {
        let _ = manifest.decrypt_filenames(&[7; 32]);
        let _ = manifest.serialize();
    }
    let _ = zip_decompress(data, true);
});
