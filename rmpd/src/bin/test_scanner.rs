use rmpd_core::event::EventBus;
use rmpd_library::{Database, Scanner};
use std::path::Path;

fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Create database
    let db = Database::open("/tmp/rmpd_test.db")?;

    // Create event bus and scanner
    let event_bus = EventBus::new();
    let scanner = Scanner::new(event_bus);

    // Scan music directory
    let music_dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/home/gianluca/Music".to_string());

    println!("Scanning: {}", music_dir);

    let stats = scanner.scan_directory(&db, Path::new(&music_dir))?;

    println!("\nScan Statistics:");
    println!("  Files scanned: {}", stats.scanned);
    println!("  Files added: {}", stats.added);
    println!("  Files updated: {}", stats.updated);
    println!("  Errors: {}", stats.errors);

    // Show database stats
    println!("\nDatabase Statistics:");
    println!("  Total songs: {}", db.count_songs()?);
    println!("  Total artists: {}", db.count_artists()?);
    println!("  Total albums: {}", db.count_albums()?);

    // List first 5 songs
    let songs = db.list_all_songs()?;
    println!("\nFirst 5 songs:");
    for song in songs.iter().take(5) {
        println!(
            "  {} - {} - {}",
            song.display_artist(),
            song.display_album(),
            song.display_title()
        );
    }

    Ok(())
}
