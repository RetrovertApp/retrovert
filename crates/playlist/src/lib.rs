//! Host-agnostic playlist storage, persistence, and playback ordering.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

const FORMAT_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PlaylistId(u64);

impl PlaylistId {
    pub const fn from_raw(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ItemId(u64);

impl ItemId {
    pub const fn from_raw(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Caller-owned fields accepted when an item is appended.
pub struct ItemSpec<P> {
    pub identity: String,
    pub subsong: u32,
    pub title: String,
    pub artist: String,
    pub duration_ms: u32,
    pub payload: P,
}

impl<P> ItemSpec<P> {
    pub fn new(identity: impl Into<String>, payload: P) -> Self {
        Self {
            identity: identity.into(),
            subsong: 0,
            title: String::new(),
            artist: String::new(),
            duration_ms: 0,
            payload,
        }
    }
}

/// One stored playlist item with an engine-minted stable id.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Item<P> {
    #[serde(skip_serializing)]
    id: ItemId,
    pub identity: String,
    pub subsong: u32,
    pub title: String,
    pub artist: String,
    pub duration_ms: u32,
    pub payload: P,
}

impl<P> Item<P> {
    pub const fn id(&self) -> ItemId {
        self.id
    }
}

pub struct Playlist<P> {
    id: PlaylistId,
    name: Option<String>,
    items: Vec<Item<P>>,
    content_revision: u64,
    structure_revision: u64,
    last_saved_revision: u64,
}

impl<P> Playlist<P> {
    pub const fn id(&self) -> PlaylistId {
        self.id
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn is_anonymous(&self) -> bool {
        self.name.is_none()
    }

    pub fn items(&self) -> &[Item<P>] {
        &self.items
    }

    pub fn item(&self, id: ItemId) -> Option<&Item<P>> {
        self.items.iter().find(|item| item.id == id)
    }

    pub const fn content_revision(&self) -> u64 {
        self.content_revision
    }

    pub const fn structure_revision(&self) -> u64 {
        self.structure_revision
    }

    pub const fn last_saved_revision(&self) -> u64 {
        self.last_saved_revision
    }

    pub const fn is_dirty(&self) -> bool {
        self.content_revision != self.last_saved_revision
    }

    fn item_index(&self, id: ItemId) -> Option<usize> {
        self.items.iter().position(|item| item.id == id)
    }

    fn touch_content(&mut self) {
        self.content_revision = self.content_revision.saturating_add(1);
    }

    fn touch_structure(&mut self) {
        self.touch_content();
        self.structure_revision = self.structure_revision.saturating_add(1);
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Mode {
    #[default]
    Sequential,
    Loop,
}

#[derive(Clone, Copy)]
enum Advance {
    Start,
    Forward,
    Backward,
}

struct Cursor {
    playlist: PlaylistId,
    current: Option<ItemId>,
    position: usize,
    pending: Option<Advance>,
    failures: usize,
    current_removed: bool,
}

#[derive(Debug)]
pub enum PersistenceError {
    Json(serde_json::Error),
    UnsupportedVersion(u32),
    UnknownPlaylist,
    IdExhausted,
}

impl fmt::Display for PersistenceError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(output, "playlist JSON: {error}"),
            Self::UnsupportedVersion(version) => {
                write!(output, "unsupported playlist version {version}")
            }
            Self::UnknownPlaylist => output.write_str("unknown playlist"),
            Self::IdExhausted => output.write_str("playlist id space exhausted"),
        }
    }
}

impl Error for PersistenceError {}

impl From<serde_json::Error> for PersistenceError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Deserialize)]
struct Document<P> {
    name: Option<String>,
    items: Vec<WireItem<P>>,
}

#[derive(Deserialize)]
struct DocumentVersion {
    version: u32,
}

#[derive(Deserialize)]
struct WireItem<P> {
    identity: String,
    subsong: u32,
    title: String,
    artist: String,
    duration_ms: u32,
    payload: P,
}

/// Ordered playlist storage and a passive playback cursor.
pub struct Engine<P> {
    playlists: Vec<Playlist<P>>,
    next_playlist_id: u64,
    next_item_id: u64,
    revision: u64,
    cursor: Option<Cursor>,
    mode: Mode,
}

impl<P> Default for Engine<P> {
    fn default() -> Self {
        Self {
            playlists: Vec::new(),
            next_playlist_id: 0,
            next_item_id: 0,
            revision: 0,
            cursor: None,
            mode: Mode::Sequential,
        }
    }
}

impl<P> Engine<P> {
    pub fn new() -> Self {
        Self::default()
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub fn playlists(&self) -> impl ExactSizeIterator<Item = &Playlist<P>> {
        self.playlists.iter()
    }

    pub fn playlist(&self, id: PlaylistId) -> Option<&Playlist<P>> {
        self.playlists.iter().find(|playlist| playlist.id == id)
    }

    pub fn item(&self, playlist: PlaylistId, item: ItemId) -> Option<&Item<P>> {
        self.playlist(playlist)?.item(item)
    }

    pub fn create(&mut self, name: Option<String>) -> Option<PlaylistId> {
        let id = self.mint_playlist_id()?;
        self.playlists.push(Playlist {
            id,
            name,
            items: Vec::new(),
            content_revision: 0,
            structure_revision: 0,
            last_saved_revision: 0,
        });
        self.touch();
        Some(id)
    }

    pub fn delete(&mut self, id: PlaylistId) -> bool {
        let Some(index) = self.playlist_index(id) else {
            return false;
        };
        if self.playing_playlist() == Some(id) {
            self.cursor = None;
        }
        self.playlists.remove(index);
        self.touch();
        true
    }

    pub fn rename(&mut self, id: PlaylistId, name: Option<String>) -> bool {
        let Some(index) = self.playlist_index(id) else {
            return false;
        };
        if self.playlists[index].name == name {
            return false;
        }
        self.playlists[index].name = name;
        self.playlists[index].touch_structure();
        self.touch();
        true
    }

    pub fn touch_structure_preserving_dirty(&mut self, id: PlaylistId) -> bool {
        let Some(index) = self.playlist_index(id) else {
            return false;
        };
        let was_dirty = self.playlists[index].is_dirty();
        self.playlists[index].touch_structure();
        if !was_dirty {
            self.playlists[index].last_saved_revision = self.playlists[index].content_revision;
        }
        self.touch();
        true
    }

    pub fn append(&mut self, playlist: PlaylistId, spec: ItemSpec<P>) -> Option<ItemId> {
        let index = self.playlist_index(playlist)?;
        let id = self.mint_item_id()?;
        self.playlists[index].items.push(Item {
            id,
            identity: spec.identity,
            subsong: spec.subsong,
            title: spec.title,
            artist: spec.artist,
            duration_ms: spec.duration_ms,
            payload: spec.payload,
        });
        self.playlists[index].touch_content();
        self.touch();
        Some(id)
    }

    pub fn edit_item(
        &mut self,
        playlist: PlaylistId,
        item: ItemId,
        edit: impl FnOnce(&mut Item<P>),
    ) -> bool {
        let Some(playlist_index) = self.playlist_index(playlist) else {
            return false;
        };
        let Some(item_index) = self.playlists[playlist_index].item_index(item) else {
            return false;
        };
        edit(&mut self.playlists[playlist_index].items[item_index]);
        self.playlists[playlist_index].touch_structure();
        self.touch();
        true
    }

    pub fn remove_item(&mut self, playlist: PlaylistId, item: ItemId) -> bool {
        let Some(playlist_index) = self.playlist_index(playlist) else {
            return false;
        };
        let Some(item_index) = self.playlists[playlist_index].item_index(item) else {
            return false;
        };
        if let Some(cursor) = self.cursor.as_mut() {
            if cursor.playlist == playlist && cursor.current == Some(item) {
                cursor.position = item_index;
                cursor.failures = 0;
                cursor.current_removed = true;
            }
        }
        self.playlists[playlist_index].items.remove(item_index);
        self.playlists[playlist_index].touch_structure();
        self.touch();
        true
    }

    /// Moves `item` to the front, or immediately after `after`.
    pub fn move_item(&mut self, playlist: PlaylistId, item: ItemId, after: Option<ItemId>) -> bool {
        if after == Some(item) {
            return false;
        }
        let Some(playlist_index) = self.playlist_index(playlist) else {
            return false;
        };
        let Some(from) = self.playlists[playlist_index].item_index(item) else {
            return false;
        };
        let destination = match after {
            Some(after) => {
                let Some(index) = self.playlists[playlist_index].item_index(after) else {
                    return false;
                };
                index + 1
            }
            None => 0,
        };
        let adjusted = if from < destination {
            destination - 1
        } else {
            destination
        };
        if adjusted == from {
            return false;
        }
        let moved = self.playlists[playlist_index].items.remove(from);
        self.playlists[playlist_index].items.insert(adjusted, moved);
        self.playlists[playlist_index].touch_structure();
        self.touch();
        true
    }

    pub fn mark_saved(&mut self, id: PlaylistId) -> bool {
        let Some(index) = self.playlist_index(id) else {
            return false;
        };
        self.playlists[index].last_saved_revision = self.playlists[index].content_revision;
        true
    }

    pub fn start(&mut self, playlist: PlaylistId, from: Option<ItemId>) -> bool {
        self.stop();
        let Some(stored) = self.playlist(playlist) else {
            return false;
        };
        if stored.items.is_empty() {
            return false;
        }
        let position = match from {
            Some(item) => {
                let Some(position) = stored.item_index(item) else {
                    return false;
                };
                position
            }
            None => 0,
        };
        self.mode = Mode::Sequential;
        self.cursor = Some(Cursor {
            playlist,
            current: None,
            position,
            pending: Some(Advance::Start),
            failures: 0,
            current_removed: false,
        });
        true
    }

    pub fn stop(&mut self) {
        let Some(cursor) = self.cursor.take() else {
            return;
        };
        if self
            .playlist(cursor.playlist)
            .is_some_and(Playlist::is_anonymous)
        {
            self.remove_playlist(cursor.playlist);
        }
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    pub const fn mode(&self) -> Mode {
        self.mode
    }

    pub fn is_playing(&self) -> bool {
        self.cursor.is_some()
    }

    pub fn playing_playlist(&self) -> Option<PlaylistId> {
        self.cursor.as_ref().map(|cursor| cursor.playlist)
    }

    pub fn current_item_id(&self) -> Option<ItemId> {
        self.cursor.as_ref().and_then(|cursor| cursor.current)
    }

    pub fn current(&self) -> Option<&Item<P>> {
        let cursor = self.cursor.as_ref()?;
        self.item(cursor.playlist, cursor.current?)
    }

    /// Pulls the item queued by `start`, transport, or a playback outcome.
    pub fn poll(&mut self) -> Option<&Item<P>> {
        let advance = self.cursor.as_ref()?.pending?;
        let Some((playlist_index, position)) = self.target_position(advance) else {
            self.stop();
            return None;
        };
        let item = self.playlists[playlist_index].items[position].id;
        let Some(cursor) = self.cursor.as_mut() else {
            unreachable!("a validated playback target has a cursor");
        };
        cursor.pending = None;
        cursor.current = Some(item);
        cursor.position = position;
        cursor.current_removed = false;
        Some(&self.playlists[playlist_index].items[position])
    }

    pub fn on_finished(&mut self) {
        let Some(cursor) = self.cursor.as_mut() else {
            return;
        };
        if cursor.current.is_some() && cursor.pending.is_none() {
            cursor.failures = 0;
            cursor.pending = Some(Advance::Forward);
        }
    }

    pub fn on_failed(&mut self) {
        let Some(cursor) = self.cursor.as_mut() else {
            return;
        };
        if cursor.current.is_none() || cursor.pending.is_some() {
            return;
        }
        if cursor.current_removed {
            cursor.failures = 0;
            cursor.pending = Some(Advance::Forward);
            return;
        }
        cursor.failures = cursor.failures.saturating_add(1);
        let playlist = cursor.playlist;
        let failures = cursor.failures;
        let item_count = self
            .playlist(playlist)
            .map_or(0, |stored| stored.items.len());
        if self.mode == Mode::Loop && failures >= item_count {
            self.stop();
        } else if let Some(cursor) = self.cursor.as_mut() {
            cursor.pending = Some(Advance::Forward);
        }
    }

    pub fn next(&mut self) {
        let Some(cursor) = self.cursor.as_mut() else {
            return;
        };
        if cursor.current.is_some() && cursor.pending.is_none() {
            cursor.failures = 0;
            cursor.pending = Some(Advance::Forward);
        }
    }

    pub fn previous(&mut self) {
        if self.previous_position().is_none() {
            return;
        }
        if let Some(cursor) = self.cursor.as_mut() {
            if cursor.pending.is_none() {
                cursor.failures = 0;
                cursor.pending = Some(Advance::Backward);
            }
        }
    }

    pub fn peek_next(&self) -> Option<&Item<P>> {
        let position = self.forward_position()?;
        let playlist = self.cursor.as_ref()?.playlist;
        self.playlist(playlist)?.items.get(position)
    }

    fn target_position(&self, advance: Advance) -> Option<(usize, usize)> {
        let cursor = self.cursor.as_ref()?;
        let playlist_index = self.playlist_index(cursor.playlist)?;
        let playlist = &self.playlists[playlist_index];
        let position = match advance {
            Advance::Start => (cursor.position < playlist.items.len()).then_some(cursor.position),
            Advance::Forward => self.forward_position(),
            Advance::Backward => self.previous_position(),
        }?;
        Some((playlist_index, position))
    }

    fn forward_position(&self) -> Option<usize> {
        let cursor = self.cursor.as_ref()?;
        let playlist = self.playlist(cursor.playlist)?;
        if playlist.items.is_empty() {
            return None;
        }
        let next = cursor
            .current
            .and_then(|current| playlist.item_index(current))
            .map_or(cursor.position, |position| position + 1);
        if next < playlist.items.len() {
            Some(next)
        } else if self.mode == Mode::Loop {
            Some(0)
        } else {
            None
        }
    }

    fn previous_position(&self) -> Option<usize> {
        let cursor = self.cursor.as_ref()?;
        let playlist = self.playlist(cursor.playlist)?;
        let position = cursor
            .current
            .and_then(|current| playlist.item_index(current))
            .unwrap_or(cursor.position);
        position.checked_sub(1)
    }

    fn playlist_index(&self, id: PlaylistId) -> Option<usize> {
        self.playlists.iter().position(|playlist| playlist.id == id)
    }

    fn mint_playlist_id(&mut self) -> Option<PlaylistId> {
        self.next_playlist_id = self.next_playlist_id.checked_add(1)?;
        Some(PlaylistId(self.next_playlist_id))
    }

    fn mint_item_id(&mut self) -> Option<ItemId> {
        self.next_item_id = self.next_item_id.checked_add(1)?;
        Some(ItemId(self.next_item_id))
    }

    fn remove_playlist(&mut self, id: PlaylistId) {
        if let Some(index) = self.playlist_index(id) {
            self.playlists.remove(index);
            self.touch();
        }
    }

    fn touch(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }
}

#[derive(Serialize)]
struct DocumentRef<'a, P> {
    version: u32,
    name: &'a Option<String>,
    items: &'a [Item<P>],
}

impl<P: Serialize> Engine<P> {
    pub fn to_bytes(&self, id: PlaylistId) -> Result<Vec<u8>, PersistenceError> {
        let playlist = self.playlist(id).ok_or(PersistenceError::UnknownPlaylist)?;
        Ok(serde_json::to_vec_pretty(&DocumentRef {
            version: FORMAT_VERSION,
            name: &playlist.name,
            items: &playlist.items,
        })?)
    }
}

impl<P: DeserializeOwned> Engine<P> {
    pub fn from_bytes(&mut self, bytes: &[u8]) -> Result<PlaylistId, PersistenceError> {
        let version: DocumentVersion = serde_json::from_slice(bytes)?;
        if version.version != FORMAT_VERSION {
            return Err(PersistenceError::UnsupportedVersion(version.version));
        }
        let document: Document<P> = serde_json::from_slice(bytes)?;
        let next_playlist_id = self
            .next_playlist_id
            .checked_add(1)
            .ok_or(PersistenceError::IdExhausted)?;
        let item_count =
            u64::try_from(document.items.len()).map_err(|_| PersistenceError::IdExhausted)?;
        let final_item_id = self
            .next_item_id
            .checked_add(item_count)
            .ok_or(PersistenceError::IdExhausted)?;
        let mut item_id = self.next_item_id;
        let items = document
            .items
            .into_iter()
            .map(|item| {
                item_id += 1;
                Item {
                    id: ItemId(item_id),
                    identity: item.identity,
                    subsong: item.subsong,
                    title: item.title,
                    artist: item.artist,
                    duration_ms: item.duration_ms,
                    payload: item.payload,
                }
            })
            .collect();
        self.next_playlist_id = next_playlist_id;
        self.next_item_id = final_item_id;
        let id = PlaylistId(next_playlist_id);
        self.playlists.push(Playlist {
            id,
            name: document.name,
            items,
            content_revision: 0,
            structure_revision: 0,
            last_saved_revision: 0,
        });
        self.touch();
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
    struct Payload {
        source: String,
        #[serde(skip)]
        transient: u32,
    }

    fn spec(identity: &str) -> ItemSpec<Payload> {
        ItemSpec {
            identity: identity.to_string(),
            subsong: 3,
            title: format!("Title {identity}"),
            artist: "Artist".to_string(),
            duration_ms: 4_200,
            payload: Payload {
                source: format!("source-{identity}"),
                transient: 7,
            },
        }
    }

    fn playlist(engine: &mut Engine<Payload>, name: Option<&str>) -> PlaylistId {
        match engine.create(name.map(str::to_string)) {
            Some(id) => id,
            None => panic!("playlist id"),
        }
    }

    fn append(engine: &mut Engine<Payload>, playlist: PlaylistId, identity: &str) -> ItemId {
        match engine.append(playlist, spec(identity)) {
            Some(id) => id,
            None => panic!("item id"),
        }
    }

    fn polled_identity(engine: &mut Engine<Payload>) -> Option<String> {
        engine.poll().map(|item| item.identity.clone())
    }

    #[test]
    fn item_ids_are_monotonic_stable_and_never_reused() {
        let mut engine = Engine::new();
        let first_playlist = playlist(&mut engine, Some("first"));
        let first = append(&mut engine, first_playlist, "a");
        let second = append(&mut engine, first_playlist, "b");
        assert!(engine.move_item(first_playlist, first, Some(second)));
        assert_eq!(
            engine.item(first_playlist, first).map(Item::id),
            Some(first)
        );
        assert!(engine.remove_item(first_playlist, first));

        let other_playlist = playlist(&mut engine, Some("other"));
        let third = append(&mut engine, other_playlist, "c");
        assert!(first.get() < second.get() && second.get() < third.get());
    }

    #[test]
    fn revisions_keep_append_on_the_fast_path_and_dirty_tracks_save() {
        let mut engine = Engine::new();
        let id = playlist(&mut engine, Some("revisions"));
        let initial = match engine.playlist(id) {
            Some(playlist) => (playlist.content_revision(), playlist.structure_revision()),
            None => panic!("playlist"),
        };
        let item = append(&mut engine, id, "a");
        let appended = match engine.playlist(id) {
            Some(playlist) => {
                assert!(playlist.is_dirty());
                (playlist.content_revision(), playlist.structure_revision())
            }
            None => panic!("playlist"),
        };
        assert_ne!(appended.0, initial.0);
        assert_eq!(appended.1, initial.1);
        assert!(engine.mark_saved(id));
        assert!(engine
            .playlist(id)
            .is_some_and(|playlist| !playlist.is_dirty()));

        assert!(engine.edit_item(id, item, |item| item.title = "Changed".to_string()));
        let edited = match engine.playlist(id) {
            Some(playlist) => (playlist.content_revision(), playlist.structure_revision()),
            None => panic!("playlist"),
        };
        assert_ne!(edited.0, appended.0);
        assert_ne!(edited.1, appended.1);
        assert!(engine.playlist(id).is_some_and(Playlist::is_dirty));

        let second = append(&mut engine, id, "b");
        let before_move = match engine.playlist(id) {
            Some(playlist) => playlist.structure_revision(),
            None => panic!("playlist"),
        };
        assert!(engine.move_item(id, second, None));
        let after_move = match engine.playlist(id) {
            Some(playlist) => playlist.structure_revision(),
            None => panic!("playlist"),
        };
        assert_ne!(after_move, before_move);
        assert!(engine.remove_item(id, second));
        assert_ne!(
            engine.playlist(id).map(Playlist::structure_revision),
            Some(after_move)
        );
    }

    #[test]
    fn rename_updates_revisions_and_dirty_state_only_when_changed() {
        let mut engine = Engine::<Payload>::new();
        let id = playlist(&mut engine, Some("before"));
        let before = match engine.playlist(id) {
            Some(playlist) => (playlist.content_revision(), playlist.structure_revision()),
            None => panic!("playlist"),
        };

        assert!(engine.rename(id, Some("after".to_string())));
        let renamed = match engine.playlist(id) {
            Some(playlist) => {
                assert_eq!(playlist.name(), Some("after"));
                assert!(playlist.is_dirty());
                (playlist.content_revision(), playlist.structure_revision())
            }
            None => panic!("playlist"),
        };
        assert_ne!(renamed.0, before.0);
        assert_ne!(renamed.1, before.1);

        assert!(!engine.rename(id, Some("after".to_string())));
        assert_eq!(
            engine
                .playlist(id)
                .map(|playlist| (playlist.content_revision(), playlist.structure_revision())),
            Some(renamed)
        );
    }

    #[test]
    fn sequential_and_loop_are_pull_driven_and_start_resets_mode() {
        let mut engine = Engine::new();
        let id = playlist(&mut engine, Some("play"));
        append(&mut engine, id, "a");
        append(&mut engine, id, "b");
        assert!(engine.start(id, None));
        assert_eq!(polled_identity(&mut engine).as_deref(), Some("a"));
        assert!(engine.poll().is_none());
        assert_eq!(
            engine.peek_next().map(|item| item.identity.as_str()),
            Some("b")
        );
        engine.on_finished();
        assert_eq!(polled_identity(&mut engine).as_deref(), Some("b"));
        engine.set_mode(Mode::Loop);
        engine.on_finished();
        assert_eq!(polled_identity(&mut engine).as_deref(), Some("a"));

        assert!(engine.start(id, None));
        assert_eq!(engine.mode(), Mode::Sequential);
    }

    #[test]
    fn failed_items_advance_and_a_failed_loop_cycle_stops() {
        let mut engine = Engine::new();
        let id = playlist(&mut engine, Some("failures"));
        append(&mut engine, id, "a");
        append(&mut engine, id, "b");
        assert!(engine.start(id, None));
        assert_eq!(polled_identity(&mut engine).as_deref(), Some("a"));
        engine.on_failed();
        assert_eq!(polled_identity(&mut engine).as_deref(), Some("b"));
        engine.on_failed();
        assert!(engine.poll().is_none());
        assert!(!engine.is_playing());

        assert!(engine.start(id, None));
        engine.set_mode(Mode::Loop);
        assert_eq!(polled_identity(&mut engine).as_deref(), Some("a"));
        engine.on_failed();
        assert_eq!(polled_identity(&mut engine).as_deref(), Some("b"));
        engine.on_failed();
        assert!(!engine.is_playing());
        assert!(engine.poll().is_none());
    }

    #[test]
    fn removed_failed_item_does_not_count_against_the_remaining_loop() {
        let mut engine = Engine::new();
        let id = playlist(&mut engine, Some("removed failure"));
        let first = append(&mut engine, id, "a");
        append(&mut engine, id, "b");
        assert!(engine.start(id, None));
        engine.set_mode(Mode::Loop);
        assert_eq!(polled_identity(&mut engine).as_deref(), Some("a"));

        assert!(engine.remove_item(id, first));
        engine.on_failed();
        assert!(engine.is_playing());
        assert_eq!(polled_identity(&mut engine).as_deref(), Some("b"));
        engine.on_failed();
        assert!(!engine.is_playing());
    }

    #[test]
    fn removing_current_advances_to_its_successor_by_position() {
        let mut engine = Engine::new();
        let id = playlist(&mut engine, Some("remove"));
        let first = append(&mut engine, id, "a");
        append(&mut engine, id, "b");
        append(&mut engine, id, "c");
        assert!(engine.start(id, Some(first)));
        assert_eq!(polled_identity(&mut engine).as_deref(), Some("a"));
        assert!(engine.remove_item(id, first));
        assert!(engine.current().is_none());
        engine.on_finished();
        assert_eq!(polled_identity(&mut engine).as_deref(), Some("b"));
    }

    #[test]
    fn deleting_the_playing_playlist_stops() {
        let mut engine = Engine::new();
        let id = playlist(&mut engine, Some("delete"));
        append(&mut engine, id, "a");
        assert!(engine.start(id, None));
        assert!(engine.delete(id));
        assert!(!engine.is_playing());
        assert!(engine.playlist(id).is_none());
        assert!(engine.poll().is_none());
    }

    #[test]
    fn stopping_auto_deletes_anonymous_playlists_only() {
        let mut engine = Engine::new();
        let anonymous = playlist(&mut engine, None);
        append(&mut engine, anonymous, "anonymous");
        assert!(engine.start(anonymous, None));
        engine.stop();
        assert!(engine.playlist(anonymous).is_none());

        let named = playlist(&mut engine, Some("named"));
        append(&mut engine, named, "named");
        assert!(engine.start(named, None));
        engine.stop();
        assert!(engine.playlist(named).is_some());
    }

    #[test]
    fn transport_and_removed_positions_are_deterministic() {
        let mut engine = Engine::new();
        let id = playlist(&mut engine, Some("transport"));
        let first = append(&mut engine, id, "a");
        append(&mut engine, id, "b");
        append(&mut engine, id, "c");
        assert!(engine.start(id, None));
        assert_eq!(polled_identity(&mut engine).as_deref(), Some("a"));
        engine.previous();
        assert!(engine.poll().is_none());
        engine.next();
        assert_eq!(polled_identity(&mut engine).as_deref(), Some("b"));
        engine.previous();
        assert_eq!(polled_identity(&mut engine).as_deref(), Some("a"));
        assert_eq!(engine.current_item_id(), Some(first));
    }

    #[test]
    fn v2_json_round_trips_vocabulary_and_nested_payload() {
        let mut engine = Engine::new();
        let id = playlist(&mut engine, Some("saved"));
        let original = append(&mut engine, id, "song");
        let bytes = match engine.to_bytes(id) {
            Ok(bytes) => bytes,
            Err(error) => panic!("serialize: {error}"),
        };
        let value: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(error) => panic!("json: {error}"),
        };
        assert_eq!(value["version"], 2);
        assert_eq!(value["name"], "saved");
        assert_eq!(value["items"][0]["identity"], "song");
        assert_eq!(value["items"][0]["subsong"], 3);
        assert_eq!(value["items"][0]["title"], "Title song");
        assert_eq!(value["items"][0]["artist"], "Artist");
        assert_eq!(value["items"][0]["duration_ms"], 4_200);
        assert_eq!(value["items"][0]["payload"]["source"], "source-song");
        assert!(value["items"][0]["payload"].get("transient").is_none());
        assert!(value["items"][0].get("id").is_none());

        let loaded_id = match engine.from_bytes(&bytes) {
            Ok(id) => id,
            Err(error) => panic!("deserialize: {error}"),
        };
        let item = match engine
            .playlist(loaded_id)
            .and_then(|playlist| playlist.items().first())
        {
            Some(item) => item,
            None => panic!("loaded item"),
        };
        assert_eq!(
            engine.playlist(loaded_id).and_then(Playlist::name),
            Some("saved")
        );
        assert_eq!(item.identity, "song");
        assert_eq!(item.title, "Title song");
        assert_eq!(item.artist, "Artist");
        assert_eq!(item.payload.source, "source-song");
        assert_eq!(item.payload.transient, 0);
        assert_ne!(item.id(), original);
        assert!(engine
            .playlist(loaded_id)
            .is_some_and(|playlist| !playlist.is_dirty()));
    }

    #[test]
    fn unsupported_json_version_is_rejected_without_mutation() {
        let mut engine = Engine::<Payload>::new();
        let result = engine.from_bytes(br#"{"version":1,"name":"old","items":[]}"#);
        assert!(matches!(
            result,
            Err(PersistenceError::UnsupportedVersion(1))
        ));
        assert!(matches!(
            engine.from_bytes(b"not json"),
            Err(PersistenceError::Json(_))
        ));
        assert_eq!(engine.playlists().len(), 0);
        assert_eq!(engine.revision(), 0);
    }

    #[test]
    fn serializing_an_unknown_playlist_reports_the_specific_error() {
        let mut engine = Engine::<Payload>::new();
        let id = playlist(&mut engine, Some("deleted"));
        assert!(engine.delete(id));
        assert!(matches!(
            engine.to_bytes(id),
            Err(PersistenceError::UnknownPlaylist)
        ));
    }
}
