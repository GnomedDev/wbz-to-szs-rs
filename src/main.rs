#[cfg(feature = "bin")]
use std::path::Path;

#[cfg(not(feature = "bin"))]
fn main() {
    eprintln!("Please compile with the `bin` feature flag!");
}

#[cfg(feature = "bin")]
fn main() -> Result<(), wbz_converter::Error> {
    use std::io::{Cursor, Read};

    let colours = fern::colors::ColoredLevelConfig::new();
    fern::Dispatch::new()
        .format(move |out, msg, rec| {
            out.finish(format_args!("[{}] {}", colours.color(rec.level()), msg));
        })
        .level(log::LevelFilter::Debug)
        .chain(std::io::stdout())
        .apply()
        .unwrap();

    let mut filename = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .expect("First argument must be a path to a file");

    let autoadd_path = Path::new("/usr/local/share/szs/auto-add/");
    let mut in_file = std::fs::File::open(&filename).unwrap();
    let (out_file, ext) = if filename.extension() == Some("u8".as_ref()) {
        let mut in_buf = Vec::new();
        let mut out_file = Cursor::new(Vec::new());
        in_file.read_to_end(&mut in_buf).unwrap();

        wbz_converter::encode_wbz(&mut in_buf, &mut out_file, autoadd_path)?;
        (out_file.into_inner(), ".wbz")
    } else {
        let out = wbz_converter::decode_wbz(
            &std::fs::File::open(&filename).expect("Unable to open WBZ file"),
            autoadd_path,
        )?;

        (out, ".u8")
    };

    // Setup new filename
    let mut stem = filename.file_stem().unwrap().to_owned();
    stem.push(ext);
    filename.set_file_name(stem);

    std::fs::write(filename, out_file).expect("Unable to write finished file");
    Ok(())
}
