use lofty::prelude::*;
use lofty::probe::Probe;

fn main() {
    let path = "/home/gianluca/Musica/01.m4a";

    println!("Testing M4A artwork extraction from: {}", path);

    match Probe::open(path) {
        Ok(probe) => {
            match probe.read() {
                Ok(tagged_file) => {
                    println!("✓ File opened successfully");
                    println!("  File type: {:?}", tagged_file.file_type());

                    // Check primary tag
                    if let Some(primary_tag) = tagged_file.primary_tag() {
                        println!("✓ Primary tag found");
                        let pictures = primary_tag.pictures();
                        println!("  Number of pictures in primary tag: {}", pictures.len());

                        for (i, pic) in pictures.iter().enumerate() {
                            println!(
                                "  Picture {}: type={:?}, mime={:?}, size={} bytes",
                                i,
                                pic.pic_type(),
                                pic.mime_type(),
                                pic.data().len()
                            );
                        }
                    } else {
                        println!("✗ No primary tag found");
                    }

                    // Check all tags
                    println!("\nChecking all tags:");
                    for tag in tagged_file.tags() {
                        println!("  Tag type: {:?}", tag.tag_type());
                        let pictures = tag.pictures();
                        println!("    Pictures in this tag: {}", pictures.len());

                        for (i, pic) in pictures.iter().enumerate() {
                            println!(
                                "    Picture {}: type={:?}, mime={:?}, size={} bytes",
                                i,
                                pic.pic_type(),
                                pic.mime_type(),
                                pic.data().len()
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!("✗ Error reading file: {}", e);
                }
            }
        }
        Err(e) => {
            eprintln!("✗ Error opening file: {}", e);
        }
    }
}
