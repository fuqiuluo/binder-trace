use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use flate2::Compression;
use flate2::GzBuilder;

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let source_path = manifest_dir.join("data/android_platform_methods.tsv");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let output_path = out_dir.join("android_platform_methods.tsv.gz");
    let source = fs::read(&source_path).expect("read android platform method table");

    println!("cargo:rerun-if-changed={}", source_path.display());
    println!(
        "cargo:rustc-env=BT_DECODER_ANDROID_PLATFORM_METHODS_HASH={:016x}",
        stable_hash(&source)
    );

    let mut encoder = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::best());
    encoder
        .write_all(&source)
        .expect("compress android platform method table");
    fs::write(
        output_path,
        encoder
            .finish()
            .expect("finish android platform method table gzip"),
    )
    .expect("write compressed android platform method table");
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
