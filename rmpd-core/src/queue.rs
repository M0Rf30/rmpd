use crate::song::Song;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueItem {
    pub id: u32,
    pub position: u32,
    pub song: Song,
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
        use rand::seq::SliceRandom;
        use rand::rng;

        self.items.shuffle(&mut rng());
        self.reindex();
        self.version += 1;
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

    fn reindex(&mut self) {
        for (idx, item) in self.items.iter_mut().enumerate() {
            item.position = idx as u32;
        }
    }
}
