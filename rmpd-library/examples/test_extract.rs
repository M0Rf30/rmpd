use rmpd_library::metadata::MetadataExtractor;
use camino::Utf8PathBuf;

fn main() {
    let path = Utf8PathBuf::from("/home/gianluca/Musica/Amon Tobin/Supermodified/01 Amon Tobin - Get Your Snack On.mp3");
    println!("Testing extraction from: {}", path);
    match MetadataExtractor::extract_from_file(&path) {
        Ok(song) => {
            println!("\nExtracted metadata:");
            println!("  Title: {:?}", song.title);
            println!("  Artist: {:?}", song.artist);
            println!("  MusicBrainz TrackID: {:?}", song.musicbrainz_trackid);
            println!("  MusicBrainz AlbumID: {:?}", song.musicbrainz_albumid);
            println!("  MusicBrainz ArtistID: {:?}", song.musicbrainz_artistid);
            println!("  ArtistSort: {:?}", song.artist_sort);
            println!("  Label: {:?}", song.label);
        }
        Err(e) => {
            eprintln!("Error extracting metadata: {}", e);
        }
    }
}
