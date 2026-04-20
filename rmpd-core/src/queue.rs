use crate::song::Song;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct QueueItem {
    pub id: u32,
    pub position: u32,
    pub song: Arc<Song>,
    /// Priority (0-255, default 0). Higher values have higher priority.
    pub priority: u8,
    /// Optional playback range (start, end) in seconds
    pub range: Option<(f64, f64)>,
    /// Custom tags attached to this queue item
    pub tags: Option<HashMap<String, String>>,
}

impl Serialize for QueueItem {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("QueueItem", 6)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("position", &self.position)?;
        state.serialize_field("song", self.song.as_ref())?;
        state.serialize_field("priority", &self.priority)?;
        state.serialize_field("range", &self.range)?;
        state.serialize_field("tags", &self.tags)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for QueueItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};
        use std::fmt;

        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "lowercase")]
        enum Field {
            Id,
            Position,
            Song,
            Priority,
            Range,
            Tags,
        }

        struct QueueItemVisitor;

        impl<'de> Visitor<'de> for QueueItemVisitor {
            type Value = QueueItem;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct QueueItem")
            }

            fn visit_map<V>(self, mut map: V) -> Result<QueueItem, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut id = None;
                let mut position = None;
                let mut song = None;
                let mut priority = None;
                let mut range = None;
                let mut tags = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Id => {
                            if id.is_some() {
                                return Err(de::Error::duplicate_field("id"));
                            }
                            id = Some(map.next_value()?);
                        }
                        Field::Position => {
                            if position.is_some() {
                                return Err(de::Error::duplicate_field("position"));
                            }
                            position = Some(map.next_value()?);
                        }
                        Field::Song => {
                            if song.is_some() {
                                return Err(de::Error::duplicate_field("song"));
                            }
                            let s: Song = map.next_value()?;
                            song = Some(Arc::new(s));
                        }
                        Field::Priority => {
                            if priority.is_some() {
                                return Err(de::Error::duplicate_field("priority"));
                            }
                            priority = Some(map.next_value()?);
                        }
                        Field::Range => {
                            if range.is_some() {
                                return Err(de::Error::duplicate_field("range"));
                            }
                            range = Some(map.next_value()?);
                        }
                        Field::Tags => {
                            if tags.is_some() {
                                return Err(de::Error::duplicate_field("tags"));
                            }
                            tags = Some(map.next_value()?);
                        }
                    }
                }

                let id = id.ok_or_else(|| de::Error::missing_field("id"))?;
                let position = position.ok_or_else(|| de::Error::missing_field("position"))?;
                let song = song.ok_or_else(|| de::Error::missing_field("song"))?;
                let priority = priority.unwrap_or(0);
                let range = range;
                let tags = tags;

                Ok(QueueItem {
                    id,
                    position,
                    song,
                    priority,
                    range,
                    tags,
                })
            }
        }

        deserializer.deserialize_struct("QueueItem", &["id", "position", "song", "priority", "range", "tags"], QueueItemVisitor)
    }
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
            song: Arc::new(song),
            priority: 0, // Default priority
            range: None, // No range restriction by default
            tags: None,  // No custom tags by default
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
            song: Arc::new(song),
            priority: 0, // Default priority
            range: None, // No range restriction by default
            tags: None,  // No custom tags by default
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

    /// Set playback range for a song with the given ID
    ///
    /// The range is specified in seconds as (start, end).
    /// Returns true if the item was found and updated.
    pub fn set_range_by_id(&mut self, id: u32, range: Option<(f64, f64)>) -> bool {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.range = range;
            self.version += 1;
            true
        } else {
            false
        }
    }

    /// Add a custom tag to a queue item
    ///
    /// Returns true if the item was found and updated.
    pub fn add_tag_by_id(&mut self, id: u32, tag: String, value: String) -> bool {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            let tags = item.tags.get_or_insert_with(HashMap::new);
            tags.insert(tag, value);
            self.version += 1;
            true
        } else {
            false
        }
    }

    /// Clear tags from a queue item
    ///
    /// If tag is Some, clears only that tag. If None, clears all tags.
    /// Returns true if the item was found.
    pub fn clear_tags_by_id(&mut self, id: u32, tag: Option<&str>) -> bool {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            if let Some(tag_name) = tag {
                // Clear specific tag
                if let Some(tags) = &mut item.tags {
                    tags.remove(tag_name);
                    // If no tags left, remove the HashMap
                    if tags.is_empty() {
                        item.tags = None;
                    }
                }
            } else {
                // Clear all tags
                item.tags = None;
            }
            self.version += 1;
            true
        } else {
            false
        }
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
    use crate::test_utils::create_test_song;

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
        assert_eq!(queue.get(0).unwrap().song.tag("title"), Some("Song 0"));
        assert_eq!(queue.get(1).unwrap().song.tag("title"), Some("Song 1"));
        assert_eq!(queue.get(5).unwrap().song.tag("title"), Some("Song 5"));
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
