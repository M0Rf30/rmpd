use camino::Utf8PathBuf;
use rmpd_library::metadata::MetadataExtractor;

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: test_m4a_artwork <path-to-audio-file>");
        return;
    };
    let path = Utf8PathBuf::from(path);

    println!("Testing artwork extraction from: {path}");

    match MetadataExtractor::extract_artwork_from_file(&path) {
        Ok(artworks) => {
            println!("Pictures found: {}", artworks.len());
            for (i, art) in artworks.iter().enumerate() {
                println!(
                    "  Picture {i}: type={}, mime={}, size={} bytes",
                    art.picture_type,
                    art.mime_type,
                    art.data.len()
                );
            }
        }
        Err(e) => eprintln!("Error reading artwork: {e}"),
    }
}
