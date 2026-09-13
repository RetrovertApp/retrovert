//! The music library any Retrovert UI lists: every file under a root that some decoder
//! claims, described by what that decoder reported about it.
//!
//! Two halves, because they run at different cadences and on different threads. [`walk`]
//! is plain filesystem work with no plugin involved, so it can run anywhere. [`Entry::describe`]
//! needs a decoder's metadata, which only the thread owning the decoders can produce; a host
//! probes one file at a time between its other duties and appends the rows as they come.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use retrovert_host::service::{MetadataValue, TrackMetadata};
use serde::Serialize;

/// One row of the library: a file and what its decoder said about it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Entry {
    /// Position in the library, stable for the library's lifetime.
    pub id: u32,
    /// Where the file is.
    pub path: PathBuf,
    /// The decoder's title, or the file stem when it gave none.
    pub title: String,
    /// The decoder's artist tag, or empty.
    pub composer: String,
    /// The name of the directory holding the file, or empty for one directly under the root.
    pub group: String,
    /// The format label: the extension, or the `mod.` style prefix, a decoder claimed.
    pub format: String,
    /// Length of the first subsong in milliseconds, or 0 when unknown.
    pub duration_ms: u32,
    /// The year from the decoder's date tag, or 0.
    pub year: u32,
    /// How many subsongs the file holds.
    pub subsongs: u32,
    /// The file's modification time as seconds since the epoch, the "added" order.
    pub added: u64,
    /// The file's size in bytes.
    pub size: u64,
}

impl Entry {
    /// Builds the row for `path` under `root`, labelled `format`, from what its decoder
    /// reported. A file no decoder described (`None`) still gets a row, named after itself.
    #[must_use]
    pub fn describe(
        id: u32,
        root: &Path,
        path: &Path,
        format: &str,
        metadata: Option<&TrackMetadata>,
    ) -> Self {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (added, size) = std::fs::metadata(path)
            .map(|m| {
                let added = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs());
                (added, m.len())
            })
            .unwrap_or((0, 0));
        let group = path
            .strip_prefix(root)
            .ok()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .map(|c| c.to_string_lossy().into_owned())
            .unwrap_or_default();

        let mut entry = Self {
            id,
            path: path.to_path_buf(),
            title: stem,
            group,
            format: format.to_ascii_uppercase(),
            subsongs: 1,
            added,
            size,
            ..Self::default()
        };
        if let Some(metadata) = metadata {
            entry.apply(metadata);
        }
        entry
    }

    fn apply(&mut self, metadata: &TrackMetadata) {
        if let Some(title) = text_tag(metadata, "title").filter(|t| !t.trim().is_empty()) {
            title.trim().clone_into(&mut self.title);
        }
        if let Some(artist) = text_tag(metadata, "artist") {
            artist.trim().clone_into(&mut self.composer);
        }
        if let Some(date) = text_tag(metadata, "date") {
            self.year = year_of(date);
        }
        self.subsongs = u32::try_from(metadata.subsongs.len().max(1)).unwrap_or(u32::MAX);
        self.duration_ms = metadata
            .subsongs
            .first()
            .and_then(|s| s.length_seconds)
            .map_or(0, seconds_to_ms)
            .max(number_tag(metadata, "length").map_or(0, seconds_to_ms));
    }
}

fn text_tag<'a>(metadata: &'a TrackMetadata, key: &str) -> Option<&'a str> {
    metadata.tags.iter().find_map(|tag| match &tag.value {
        MetadataValue::Text(text) if tag.key.eq_ignore_ascii_case(key) => Some(text.text.as_str()),
        _ => None,
    })
}

fn number_tag(metadata: &TrackMetadata, key: &str) -> Option<f64> {
    metadata.tags.iter().find_map(|tag| match tag.value {
        MetadataValue::Number(n) if tag.key.eq_ignore_ascii_case(key) => Some(n),
        _ => None,
    })
}

/// Milliseconds from a decoder's seconds; a negative or absurd length reads as unknown.
#[must_use]
pub fn seconds_to_ms(seconds: impl Into<f64>) -> u32 {
    let seconds: f64 = seconds.into();
    if !(0.0..=1.0e6).contains(&seconds) {
        return 0;
    }
    // Bounded above, so the cast cannot truncate.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let ms = (seconds * 1000.0).round() as u32;
    ms
}

/// The first four-digit run of a date tag, as a year.
fn year_of(date: &str) -> u32 {
    let digits: Vec<u32> = date.chars().map(|c| c.to_digit(10).unwrap_or(10)).collect();
    digits
        .windows(4)
        .find(|w| w.iter().all(|&d| d < 10))
        .map_or(0, |w| w.iter().fold(0, |acc, &d| acc * 10 + d))
}

/// The extension of `path` in lower case, or empty.
#[must_use]
pub fn extension(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// The `mod` of an Amiga-style `mod.name`, in lower case: the part before the first dot,
/// when there is a dot and something before it.
#[must_use]
pub fn prefix(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.split_once('.'))
        .filter(|(head, tail)| !head.is_empty() && !tail.is_empty())
        .map(|(head, _)| head.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Every regular file under `root`, recursively, in a stable order: directories before
/// their siblings' contents is not promised, only that the same tree walks the same way twice.
/// Symlinks are not followed, so a loop in the tree cannot run forever.
#[must_use]
pub fn walk(root: &Path) -> Vec<PathBuf> {
    fn visit(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            let Ok(kind) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if kind.is_dir() {
                visit(&path, out);
            } else if kind.is_file() {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    visit(root, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use retrovert_host::service::{MetadataSubsong, MetadataTag, MetadataText};

    fn text(s: &str) -> MetadataText {
        MetadataText {
            text: s.to_owned(),
            raw: None,
        }
    }

    fn tag(key: &str, value: &str) -> MetadataTag {
        MetadataTag {
            key: key.to_owned(),
            raw_key: None,
            value: MetadataValue::Text(text(value)),
        }
    }

    #[test]
    fn walk_lists_files_under_every_directory_in_a_stable_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("b/inner")).expect("mkdir");
        std::fs::write(dir.path().join("b/inner/z.mod"), b"").expect("write");
        std::fs::write(dir.path().join("a.xm"), b"").expect("write");
        std::fs::write(dir.path().join("b/y.it"), b"").expect("write");
        let first = walk(dir.path());
        let names: Vec<_> = first
            .iter()
            .map(|p| {
                p.strip_prefix(dir.path())
                    .expect("under root")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, ["a.xm", "b/inner/z.mod", "b/y.it"]);
        assert_eq!(walk(dir.path()), first);
        assert!(walk(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn a_file_no_decoder_described_is_named_after_itself() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("Party 2025")).expect("mkdir");
        let path = dir.path().join("Party 2025/neon overdrive.XM");
        std::fs::write(&path, b"12345").expect("write");
        let entry = Entry::describe(3, dir.path(), &path, "xm", None);
        assert_eq!(entry.id, 3);
        assert_eq!(entry.title, "neon overdrive");
        assert_eq!(entry.format, "XM");
        assert_eq!(entry.group, "Party 2025");
        assert_eq!(entry.size, 5);
        assert_eq!(entry.subsongs, 1);
        assert!(entry.added > 0);
        assert_eq!(
            Entry::describe(0, dir.path(), &dir.path().join("root.mod"), "mod", None).group,
            ""
        );
        std::fs::create_dir_all(dir.path().join("Party 2025/deeper")).expect("mkdir");
        let deep = dir.path().join("Party 2025/deeper/mod.thing");
        std::fs::write(&deep, b"").expect("write");
        assert_eq!(
            Entry::describe(1, dir.path(), &deep, "mod", None).group,
            "deeper"
        );
    }

    #[test]
    fn the_decoders_tags_fill_the_row() {
        let metadata = TrackMetadata {
            tags: vec![
                tag("title", "  Phosphor Dreams "),
                tag("artist", "Tesseract"),
                tag("song_type", "Impulse Tracker"),
                tag("date", "released 1999-04-01"),
            ],
            subsongs: vec![
                MetadataSubsong {
                    index: 0,
                    name: text(""),
                    length_seconds: Some(252.4),
                },
                MetadataSubsong {
                    index: 1,
                    name: text(""),
                    length_seconds: None,
                },
            ],
            ..TrackMetadata::default()
        };
        let entry = Entry::describe(
            0,
            Path::new("/lib"),
            Path::new("/lib/x/tsr.it"),
            "it",
            Some(&metadata),
        );
        assert_eq!(entry.title, "Phosphor Dreams");
        assert_eq!(entry.composer, "Tesseract");
        assert_eq!(entry.format, "IT");
        assert_eq!(entry.year, 1999);
        assert_eq!(entry.duration_ms, 252_400);
        assert_eq!(entry.subsongs, 2);
        assert_eq!(entry.group, "x");
    }

    #[test]
    fn a_length_tag_is_the_fallback_and_names_split_by_extension_or_prefix() {
        let metadata = TrackMetadata {
            tags: vec![MetadataTag {
                key: "length".to_owned(),
                raw_key: None,
                value: MetadataValue::Number(61.5),
            }],
            ..TrackMetadata::default()
        };
        let entry = Entry::describe(
            0,
            Path::new("/"),
            Path::new("/c.psid"),
            "sid",
            Some(&metadata),
        );
        assert_eq!(entry.format, "SID");
        assert_eq!(entry.duration_ms, 61_500);
        assert_eq!(entry.year, 0);
        assert_eq!(extension(Path::new("/c.PSID")), "psid");
        assert_eq!(prefix(Path::new("/x/MOD.stardust")), "mod");
        assert_eq!(prefix(Path::new("/x/plain")), "");
        assert_eq!(prefix(Path::new("/x/.hidden")), "");
    }
}
