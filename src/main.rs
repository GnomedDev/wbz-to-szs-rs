#[cfg(feature = "bin")]
use std::path::Path;

#[cfg(not(feature = "bin"))]
fn main() {
    eprintln!("Please compile with the `bin` feature flag!");
}

#[cfg(feature = "bin")]
fn main() -> Result<(), wbz_converter::Error> {
    let colours = fern::colors::ColoredLevelConfig::new();
    fern::Dispatch::new()
        .format(move |out, msg, rec| {
            out.finish(format_args!("[{}] {}", colours.color(rec.level()), msg));
        })
        .level(log::LevelFilter::Info)
        .chain(std::io::stdout())
        .apply()
        .unwrap();

    let mut filename = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .expect("First argument must be a path to a WBZ file");

    let u8_file = wbz_converter::decode_wbz(
        std::fs::File::open(&filename).expect("Unable to open WBZ file"),
        Path::new("/usr/local/share/szs/auto-add/"),
    )?;

    // Setup new filename
    let mut stem = filename.file_stem().unwrap().to_owned();
    stem.push(".u8");
    filename.set_file_name(stem);

    std::fs::write(filename, u8_file).expect("Unable to write finished file");
    Ok(())
}
