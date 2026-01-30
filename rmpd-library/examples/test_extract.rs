use rmpd_library::metadata::MetadataExtractor;
use camino::Utf8PathBuf;

fn main() {
    // Test with DSD file
    let dsd_path = Utf8PathBuf::from("/home/gianluca/Musica/Back in N.Y.C..dsf");
    println!("Testing DSD extraction from: {}", dsd_path);
    match MetadataExtractor::extract_from_file(&dsd_path) {
        Ok(song) => {
            println!("\nExtracted DSD metadata:");
            println!("  Title: {:?}", song.title);
            println!("  Artist: {:?}", song.artist);
            println!("  Album: {:?}", song.album);
            println!("  Date: {:?}", song.date);
            println!("  Genre: {:?}", song.genre);
            println!("  Sample Rate: {:?}", song.sample_rate);
            println!("  Channels: {:?}", song.channels);
            println!("  Bits Per Sample: {:?}", song.bits_per_sample);
            println!("  Duration: {:?}", song.duration);
            println!("  MusicBrainz TrackID: {:?}", song.musicbrainz_trackid);
        }
        Err(e) => {
            eprintln!("Error extracting DSD metadata: {}", e);
        }
    }

    println!("\n---\n");

    // Test with MP3 file
    let mp3_path = Utf8PathBuf::from("/home/gianluca/Musica/Amon Tobin/Supermodified/01 Amon Tobin - Get Your Snack On.mp3");
    println!("Testing MP3 extraction from: {}", mp3_path);
    match MetadataExtractor::extract_from_file(&mp3_path) {
        Ok(song) => {
            println!("\nExtracted MP3 metadata:");
            println!("  Title: {:?}", song.title);
            println!("  Artist: {:?}", song.artist);
            println!("  Album: {:?}", song.album);
            println!("  MusicBrainz TrackID: {:?}", song.musicbrainz_trackid);
        }
        Err(e) => {
            eprintln!("Error extracting MP3 metadata: {}", e);
        }
    }
}
