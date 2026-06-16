use camino::Utf8PathBuf;
use rmpd_library::metadata::MetadataExtractor;

/// Manual helper: extract and print metadata for one or more audio files.
///
/// Usage: `cargo run -p rmpd-library --example test_extract -- <file> [<file> ...]`
fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("usage: test_extract <audio-file> [<audio-file> ...]");
        std::process::exit(2);
    }

    for arg in files {
        let path = Utf8PathBuf::from(&arg);
        println!("Testing extraction from: {path}");
        match MetadataExtractor::extract_from_file(&path) {
            Ok(song) => {
                println!("  Title: {:?}", song.tag("title"));
                println!("  Artist: {:?}", song.tag("artist"));
                println!("  Album: {:?}", song.tag("album"));
                println!("  Date: {:?}", song.tag("date"));
                println!("  Genre: {:?}", song.tag("genre"));
                println!("  Sample Rate: {:?}", song.sample_rate);
                println!("  Channels: {:?}", song.channels);
                println!("  Bits Per Sample: {:?}", song.bits_per_sample);
                println!("  Duration: {:?}", song.duration);
                println!(
                    "  MusicBrainz TrackID: {:?}",
                    song.tag("musicbrainz_trackid")
                );
            }
            Err(e) => eprintln!("  Error extracting metadata: {e}"),
        }
        println!("\n---\n");
    }
}
