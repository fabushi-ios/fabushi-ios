//! Target Symbol: DataFolder
//! Telegram Desktop 1:1 Submodule: `DataFolder`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_peer_id::PeerId;

pub type FolderId = i32;

/// Archive or customized chat folder container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub id: FolderId,
    pub title: String,
    pub peers: Vec<PeerId>,
    pub pinned: Vec<PeerId>,
}

impl Folder {
    pub const ARCHIVE_ID: FolderId = 1;

    pub fn new(id: FolderId, title: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
            peers: Vec::new(),
            pinned: Vec::new(),
        }
    }

    pub fn is_archive(&self) -> bool {
        self.id == Self::ARCHIVE_ID
    }

    pub fn add_peer(&mut self, peer_id: PeerId) {
        if !self.peers.contains(&peer_id) {
            self.peers.push(peer_id);
        }
    }

    pub fn remove_peer(&mut self, peer_id: PeerId) {
        self.peers.retain(|&p| p != peer_id);
        self.pinned.retain(|&p| p != peer_id);
    }

    pub fn pin_peer(&mut self, peer_id: PeerId) {
        self.add_peer(peer_id);
        if !self.pinned.contains(&peer_id) {
            self.pinned.push(peer_id);
        }
    }

    pub fn unpin_peer(&mut self, peer_id: PeerId) {
        self.pinned.retain(|&p| p != peer_id);
    }

    pub fn contains(&self, peer_id: PeerId) -> bool {
        self.peers.contains(&peer_id)
    }
}

/// Target Symbol for Module 03 Ledger: DataFolder coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataFolder {
    pub folders: Vec<Folder>,
}

impl DataFolder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_or_create(&mut self, id: FolderId, title: &str) -> &mut Folder {
        if let Some(pos) = self.folders.iter().position(|f| f.id == id) {
            &mut self.folders[pos]
        } else {
            self.folders.push(Folder::new(id, title));
            self.folders.last_mut().unwrap()
        }
    }

    pub fn find(&self, id: FolderId) -> Option<&Folder> {
        self.folders.iter().find(|f| f.id == id)
    }

    pub fn count(&self) -> usize {
        self.folders.len()
    }
}
