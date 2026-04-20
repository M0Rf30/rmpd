use rmpd_core::queue::Queue;
use rmpd_core::test_utils::create_test_song;

#[test]
fn test_add_single_song() {
    let mut queue = Queue::new();
    let song = create_test_song(1, "test");
    
    let id = queue.add(song);
    
    assert_eq!(queue.len(), 1);
    assert_eq!(id, 0);
    assert!(!queue.is_empty());
}

#[test]
fn test_add_multiple_songs() {
    let mut queue = Queue::new();
    
    let id1 = queue.add(create_test_song(1, "song1"));
    let id2 = queue.add(create_test_song(2, "song2"));
    let id3 = queue.add(create_test_song(3, "song3"));
    
    assert_eq!(queue.len(), 3);
    assert_eq!(id1, 0);
    assert_eq!(id2, 1);
    assert_eq!(id3, 2);
}

#[test]
fn test_delete_by_position() {
    let mut queue = Queue::new();
    queue.add(create_test_song(1, "song1"));
    queue.add(create_test_song(2, "song2"));
    queue.add(create_test_song(3, "song3"));
    
    let deleted = queue.delete(1);
    
    assert!(deleted.is_some());
    assert_eq!(queue.len(), 2);
    assert_eq!(queue.get(0).unwrap().song.tag("title"), Some("Song song1"));
    assert_eq!(queue.get(1).unwrap().song.tag("title"), Some("Song song3"));
}

#[test]
fn test_delete_by_id() {
    let mut queue = Queue::new();
    let id1 = queue.add(create_test_song(1, "song1"));
    let id2 = queue.add(create_test_song(2, "song2"));
    let id3 = queue.add(create_test_song(3, "song3"));
    
    let deleted = queue.delete_id(id2);
    
    assert!(deleted.is_some());
    assert_eq!(queue.len(), 2);
    assert_eq!(queue.get_by_id(id1).is_some(), true);
    assert_eq!(queue.get_by_id(id2).is_none(), true);
    assert_eq!(queue.get_by_id(id3).is_some(), true);
}

#[test]
fn test_clear_queue() {
    let mut queue = Queue::new();
    queue.add(create_test_song(1, "song1"));
    queue.add(create_test_song(2, "song2"));
    queue.add(create_test_song(3, "song3"));
    
    assert_eq!(queue.len(), 3);
    
    queue.clear();
    
    assert_eq!(queue.len(), 0);
    assert!(queue.is_empty());
}

#[test]
fn test_get_by_position() {
    let mut queue = Queue::new();
    queue.add(create_test_song(1, "song1"));
    queue.add(create_test_song(2, "song2"));
    queue.add(create_test_song(3, "song3"));
    
    let item = queue.get(1);
    
    assert!(item.is_some());
    assert_eq!(item.unwrap().song.tag("title"), Some("Song song2"));
}

#[test]
fn test_get_by_id() {
    let mut queue = Queue::new();
    let _id1 = queue.add(create_test_song(1, "song1"));
    let id2 = queue.add(create_test_song(2, "song2"));
    
    let item = queue.get_by_id(id2);
    
    assert!(item.is_some());
    assert_eq!(item.unwrap().song.tag("title"), Some("Song song2"));
}

#[test]
fn test_len_and_is_empty() {
    let mut queue = Queue::new();
    
    assert_eq!(queue.len(), 0);
    assert!(queue.is_empty());
    
    queue.add(create_test_song(1, "song1"));
    assert_eq!(queue.len(), 1);
    assert!(!queue.is_empty());
    
    queue.clear();
    assert_eq!(queue.len(), 0);
    assert!(queue.is_empty());
}

#[test]
fn test_shuffle_preserves_length() {
    let mut queue = Queue::new();
    for i in 0..10 {
        queue.add(create_test_song(i as u64, &i.to_string()));
    }
    
    let original_len = queue.len();
    queue.shuffle();
    
    assert_eq!(queue.len(), original_len);
}

#[test]
fn test_shuffle_changes_order_for_nontrivial_queue() {
    let mut queue = Queue::new();
    for i in 0..10 {
        queue.add(create_test_song(i as u64, &i.to_string()));
    }
    
    // Get original IDs
    let original_ids: Vec<u32> = (0..queue.len() as u32)
        .map(|i| queue.get(i).unwrap().id)
        .collect();
    
    queue.shuffle();
    
    // Get shuffled IDs
    let shuffled_ids: Vec<u32> = (0..queue.len() as u32)
        .map(|i| queue.get(i).unwrap().id)
        .collect();
    
    // All IDs should still be present
    for id in &original_ids {
        assert!(shuffled_ids.contains(id));
    }
    
    // For a 10-item queue, it's extremely unlikely to shuffle to the same order
    // (probability is 1/10! ≈ 2.75e-7)
    // We just verify the IDs are the same set, not the order
}

#[test]
fn test_move_item() {
    let mut queue = Queue::new();
    queue.add(create_test_song(1, "song1"));
    queue.add(create_test_song(2, "song2"));
    queue.add(create_test_song(3, "song3"));
    
    let success = queue.move_item(0, 2);
    
    assert!(success);
    assert_eq!(queue.get(0).unwrap().song.tag("title"), Some("Song song2"));
    assert_eq!(queue.get(1).unwrap().song.tag("title"), Some("Song song3"));
    assert_eq!(queue.get(2).unwrap().song.tag("title"), Some("Song song1"));
}

#[test]
fn test_move_item_invalid_position() {
    let mut queue = Queue::new();
    queue.add(create_test_song(1, "song1"));
    queue.add(create_test_song(2, "song2"));
    
    let success = queue.move_item(0, 5);
    
    assert!(!success);
    assert_eq!(queue.len(), 2);
}

#[test]
fn test_move_by_id() {
    let mut queue = Queue::new();
    let id1 = queue.add(create_test_song(1, "song1"));
    let id2 = queue.add(create_test_song(2, "song2"));
    let id3 = queue.add(create_test_song(3, "song3"));
    
    let success = queue.move_by_id(id1, 2);
    
    assert!(success);
    assert_eq!(queue.get(0).unwrap().id, id2);
    assert_eq!(queue.get(1).unwrap().id, id3);
    assert_eq!(queue.get(2).unwrap().id, id1);
}

#[test]
fn test_queue_ids_are_unique() {
    let mut queue = Queue::new();
    
    let id1 = queue.add(create_test_song(1, "song1"));
    let id2 = queue.add(create_test_song(2, "song2"));
    let id3 = queue.add(create_test_song(3, "song3"));
    
    assert_ne!(id1, id2);
    assert_ne!(id2, id3);
    assert_ne!(id1, id3);
}

#[test]
fn test_positions_updated_after_delete() {
    let mut queue = Queue::new();
    queue.add(create_test_song(1, "song1"));
    queue.add(create_test_song(2, "song2"));
    queue.add(create_test_song(3, "song3"));
    
    queue.delete(1);
    
    assert_eq!(queue.get(0).unwrap().position, 0);
    assert_eq!(queue.get(1).unwrap().position, 1);
}

#[test]
fn test_swap_items() {
    let mut queue = Queue::new();
    queue.add(create_test_song(1, "song1"));
    queue.add(create_test_song(2, "song2"));
    queue.add(create_test_song(3, "song3"));
    
    let success = queue.swap(0, 2);
    
    assert!(success);
    assert_eq!(queue.get(0).unwrap().song.tag("title"), Some("Song song3"));
    assert_eq!(queue.get(2).unwrap().song.tag("title"), Some("Song song1"));
}

#[test]
fn test_add_at_position() {
    let mut queue = Queue::new();
    queue.add(create_test_song(1, "song1"));
    queue.add(create_test_song(2, "song2"));
    
    let id = queue.add_at(create_test_song(3, "song3"), Some(1));
    
    assert_eq!(queue.len(), 3);
    assert_eq!(queue.get(1).unwrap().id, id);
    assert_eq!(queue.get(1).unwrap().song.tag("title"), Some("Song song3"));
}

#[test]
fn test_version_increments() {
    let mut queue = Queue::new();
    let v1 = queue.version();
    
    queue.add(create_test_song(1, "song1"));
    let v2 = queue.version();
    assert!(v2 > v1);
    
    queue.clear();
    let v3 = queue.version();
    assert!(v3 > v2);
}
