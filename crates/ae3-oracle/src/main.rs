//! `ae3-oracle census <bundle>...` lists every `Texture2D` in the given Bundle
//! files with its size and format, and whether `ae3-core` decodes it; this is
//! how fixture candidates for each texture format are found.
//!
//! `ae3-oracle compare <bundle>...` (with the `oracle` feature) also decodes
//! each texture with `unity-rs-core` and prints how far the two disagree.

use std::path::PathBuf;
use std::process::ExitCode;

use ae3_oracle::{ImageStats, decode_with_ae3};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_default();
    let files: Vec<PathBuf> = args.map(PathBuf::from).collect();
    match command.as_str() {
        "census" => census(&files),
        #[cfg(feature = "oracle")]
        "compare" => compare(&files),
        _ => {
            eprintln!("usage: ae3-oracle census <bundle>...");
            #[cfg(feature = "oracle")]
            eprintln!("       ae3-oracle compare <bundle>...");
            ExitCode::FAILURE
        }
    }
}

/// Prints `bundle  texture  format  WxH  outcome`, one line per texture.
fn census(files: &[PathBuf]) -> ExitCode {
    let mut status = ExitCode::SUCCESS;
    for file in files {
        let textures = match decode_with_ae3(file) {
            Ok(textures) => textures,
            Err(e) => {
                eprintln!("{}: {e}", file.display());
                status = ExitCode::FAILURE;
                continue;
            }
        };
        let bundle = file.file_stem().unwrap_or_default().to_string_lossy();
        for t in textures {
            let outcome = match &t.image {
                Ok(image) => {
                    let s = ImageStats::of(image);
                    format!(
                        "ok mean={:.1},{:.1},{:.1},{:.1} opaque={:.3} distinct={}",
                        s.mean[0], s.mean[1], s.mean[2], s.mean[3], s.opaque, s.distinct
                    )
                }
                Err(e) => {
                    status = ExitCode::FAILURE;
                    format!("error: {e}")
                }
            };
            println!(
                "{bundle}\t{}\t{}\t{}x{}\t{outcome}",
                t.name,
                t.format_name(),
                t.width,
                t.height
            );
        }
    }
    status
}

/// Prints `bundle  texture  format  max  mean  differing/pixels`.
#[cfg(feature = "oracle")]
fn compare(files: &[PathBuf]) -> ExitCode {
    use ae3_oracle::PixelDiff;
    use ae3_oracle::unity_rs::decode_with_unity_rs;

    let mut status = ExitCode::SUCCESS;
    for file in files {
        let (ours, theirs) = match (decode_with_ae3(file), decode_with_unity_rs(file)) {
            (Ok(ours), Ok(theirs)) => (ours, theirs),
            (ours, theirs) => {
                eprintln!(
                    "{}: ae3 {:?}, unity-rs {:?}",
                    file.display(),
                    ours.err().map(|e| e.to_string()),
                    theirs.err()
                );
                status = ExitCode::FAILURE;
                continue;
            }
        };
        let bundle = file.file_stem().unwrap_or_default().to_string_lossy();
        for t in ours {
            let verdict = match (&t.image, theirs.get(&t.path_id)) {
                (Ok(a), Some(Ok(b))) => PixelDiff::between(a, b).map_or_else(
                    || {
                        format!(
                            "size {}x{} vs {}x{}",
                            a.width(),
                            a.height(),
                            b.width(),
                            b.height()
                        )
                    },
                    |d| {
                        format!(
                            "max={} mean={:.3} {}/{}",
                            d.max, d.mean, d.differing, d.pixels
                        )
                    },
                ),
                (a, b) => format!("ae3 {:?} unity-rs {:?}", a.as_ref().err(), b),
            };
            println!("{bundle}\t{}\t{}\t{verdict}", t.name, t.format_name());
        }
    }
    status
}
