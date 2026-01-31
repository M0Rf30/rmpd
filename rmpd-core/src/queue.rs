use crate::song::Song;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueItem {
    pub id: u32,
    pub position: u32,
    pub song: Song,
    /// Priority (0-255, default 0). Higher values have higher priority.
    pub priority: u8,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Queue {
    items: Vec<QueueItem>,
    next_id: u32,
    version: u32,
}

impl Queue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, song: Song) -> u32 {
        let id = self.next_id;
        self.next_id += 1;

        let position = self.items.len() as u32;
        self.items.push(QueueItem {
            id,
            position,
            song,
            priority: 0, // Default priority
        });

        self.version += 1;
        id
    }

    pub fn delete(&mut self, position: u32) -> Option<QueueItem> {
        if (position as usize) < self.items.len() {
            let item = self.items.remove(position as usize);
            self.reindex();
            self.version += 1;
            Some(item)
        } else {
            None
        }
    }

    pub fn delete_id(&mut self, id: u32) -> Option<QueueItem> {
        if let Some(idx) = self.items.iter().position(|item| item.id == id) {
            let item = self.items.remove(idx);
            self.reindex();
            self.version += 1;
            Some(item)
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.version += 1;
    }

    pub fn get(&self, position: u32) -> Option<&QueueItem> {
        self.items.get(position as usize)
    }

    pub fn get_by_id(&self, id: u32) -> Option<&QueueItem> {
        self.items.iter().find(|item| item.id == id)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn items(&self) -> &[QueueItem] {
        &self.items
    }

    pub fn shuffle(&mut self) {
        use rand::rng;
        use rand::seq::SliceRandom;

        self.items.shuffle(&mut rng());
        self.reindex();
        self.version += 1;
    }

    pub fn shuffle_range(&mut self, start: u32, end: u32) {
        use rand::rng;
        use rand::seq::SliceRandom;

        let start_idx = start as usize;
        let end_idx = end.min(self.items.len() as u32) as usize;

        if start_idx < end_idx && end_idx <= self.items.len() {
            self.items[start_idx..end_idx].shuffle(&mut rng());
            self.reindex();
            self.version += 1;
        }
    }

    pub fn move_item(&mut self, from: u32, to: u32) -> bool {
        if from >= self.items.len() as u32 || to >= self.items.len() as u32 {
            return false;
        }

        let item = self.items.remove(from as usize);
        self.items.insert(to as usize, item);
        self.reindex();
        self.version += 1;
        true
    }

    pub fn move_by_id(&mut self, id: u32, to: u32) -> bool {
        if let Some(from_idx) = self.items.iter().position(|i| i.id == id) {
            if to as usize > self.items.len() {
                return false;
            }
            let item = self.items.remove(from_idx);
            self.items.insert(to as usize, item);
            self.reindex();
            self.version += 1;
            true
        } else {
            false
        }
    }

    pub fn swap(&mut self, pos1: u32, pos2: u32) -> bool {
        if pos1 >= self.items.len() as u32 || pos2 >= self.items.len() as u32 {
            return false;
        }
        self.items.swap(pos1 as usize, pos2 as usize);
        self.reindex();
        self.version += 1;
        true
    }

    pub fn swap_by_id(&mut self, id1: u32, id2: u32) -> bool {
        let idx1 = self.items.iter().position(|i| i.id == id1);
        let idx2 = self.items.iter().position(|i| i.id == id2);

        if let (Some(i1), Some(i2)) = (idx1, idx2) {
            self.items.swap(i1, i2);
            self.reindex();
            self.version += 1;
            true
        } else {
            false
        }
    }

    pub fn add_at(&mut self, song: Song, position: Option<u32>) -> u32 {
        let id = self.next_id;
        self.next_id += 1;

        let pos = position.unwrap_or(self.items.len() as u32);
        let item = QueueItem {
            id,
            position: pos,
            song,
            priority: 0, // Default priority
        };

        if pos as usize >= self.items.len() {
            self.items.push(item);
        } else {
            self.items.insert(pos as usize, item);
        }

        self.reindex();
        self.version += 1;
        id
    }

    /// Set priority for songs in the given position range
    pub fn set_priority_range(&mut self, priority: u8, ranges: &[(u32, u32)]) {
        for &(start, end) in ranges {
            let start_idx = start as usize;
            let end_idx = end.min(self.items.len() as u32) as usize;

            for idx in start_idx..end_idx {
                if idx < self.items.len() {
                    self.items[idx].priority = priority;
                }
            }
        }
        self.version += 1;
    }

    /// Set priority for songs with the given IDs
    pub fn set_priority_ids(&mut self, priority: u8, ids: &[u32]) -> bool {
        let mut any_changed = false;
        for &id in ids {
            if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
                item.priority = priority;
                any_changed = true;
            }
        }
        if any_changed {
            self.version += 1;
        }
        any_changed
    }

    /// Get mutable reference to an item by ID
    pub fn get_by_id_mut(&mut self, id: u32) -> Option<&mut QueueItem> {
        self.items.iter_mut().find(|item| item.id == id)
    }

    fn reindex(&mut self) {
        for (idx, item) in self.items.iter_mut().enumerate() {
            item.position = idx as u32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::song::Song;
    use camino::Utf8PathBuf;

    fn create_test_song(id: u64, name: &str) -> Song {
        Song {
            id,
            path: Utf8PathBuf::from(format!("song{}.mp3", name)),
            title: Some(format!("Song {}", name)),
            duration: None,
            artist: None,
            album: None,
            album_artist: None,
            track: None,
            disc: None,
            date: None,
            genre: None,
            composer: None,
            performer: None,
            comment: None,
            musicbrainz_trackid: None,
            musicbrainz_albumid: None,
            musicbrainz_artistid: None,
            musicbrainz_albumartistid: None,
            musicbrainz_releasegroupid: None,
            musicbrainz_releasetrackid: None,
            artist_sort: None,
            album_artist_sort: None,
            original_date: None,
            label: None,
            sample_rate: None,
            channels: None,
            bits_per_sample: None,
            bitrate: None,
            replay_gain_track_gain: None,
            replay_gain_track_peak: None,
            replay_gain_album_gain: None,
            replay_gain_album_peak: None,
            added_at: 0,
            last_modified: 0,
        }
    }

    #[test]
    fn test_shuffle_range() {
        let mut queue = Queue::new();

        // Add test items
        for i in 0..10 {
            let song = create_test_song(i as u64, &i.to_string());
            queue.add(song);
        }

        // Get original IDs of items 2-5
        let original_ids: Vec<u32> = (2..5).map(|i| queue.get(i).unwrap().id).collect();

        // Shuffle range 2:5
        queue.shuffle_range(2, 5);

        // Get IDs after shuffle
        let shuffled_ids: Vec<u32> = (2..5).map(|i| queue.get(i).unwrap().id).collect();

        // Check that the same IDs are still in the range (just reordered)
        assert_eq!(original_ids.len(), shuffled_ids.len());
        for id in original_ids {
            assert!(shuffled_ids.contains(&id));
        }

        // Check that items outside the range weren't affected
        assert_eq!(queue.get(0).unwrap().song.title, Some("Song 0".to_string()));
        assert_eq!(queue.get(1).unwrap().song.title, Some("Song 1".to_string()));
        assert_eq!(queue.get(5).unwrap().song.title, Some("Song 5".to_string()));
    }

    #[test]
    fn test_shuffle_range_bounds() {
        let mut queue = Queue::new();

        // Add 5 items
        for i in 0..5 {
            let song = create_test_song(i as u64, &i.to_string());
            queue.add(song);
        }

        // Test with range beyond length - should handle gracefully
        queue.shuffle_range(2, 100);

        // Should still have 5 items
        assert_eq!(queue.len(), 5);
    }
}
