//! What a check decided would have to be fetched, and what it would cost.

use retrovert_tuf::manifest::{Artifact, Manifest};

use crate::transport::{ArtifactDigest, Cache};

/// The artifacts of one generation that apply to this consumer, with the byte
/// counts a caller on a metered connection decides from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The generation the channel is offering.
    pub generation_id: String,
    /// The artifacts that apply to this consumer, in manifest order.
    pub artifacts: Vec<Artifact>,
    /// What those artifacts weigh in total.
    pub total_bytes: u64,
    /// How much of that weight the cache already holds.
    pub cached_bytes: u64,
}

impl Plan {
    /// How many artifacts the plan covers.
    #[must_use]
    pub fn artifact_count(&self) -> usize {
        self.artifacts.len()
    }

    /// What applying the plan would pull over the network.
    #[must_use]
    pub fn bytes_to_fetch(&self) -> u64 {
        self.total_bytes.saturating_sub(self.cached_bytes)
    }

    pub(super) fn select(
        generation_id: String,
        manifest: &Manifest,
        target: Option<&str>,
        cache: &Cache,
    ) -> Self {
        let artifacts: Vec<Artifact> = manifest
            .artifacts
            .iter()
            .filter(|artifact| applies_to(artifact, target))
            .cloned()
            .collect();

        let mut total_bytes = 0u64;
        let mut cached_bytes = 0u64;
        for artifact in &artifacts {
            total_bytes = total_bytes.saturating_add(artifact.size);
            if is_cached(cache, artifact) {
                cached_bytes = cached_bytes.saturating_add(artifact.size);
            }
        }

        Self {
            generation_id,
            artifacts,
            total_bytes,
            cached_bytes,
        }
    }
}

/// Whether `artifact` applies to a consumer configured for `target`.
///
/// An artifact naming no target applies to every consumer, which is how one
/// channel serves both plugin sets and databases. A consumer with no target
/// configured therefore takes the untargeted artifacts and nothing else.
#[must_use]
pub(super) fn applies_to(artifact: &Artifact, target: Option<&str>) -> bool {
    match artifact.target.as_deref() {
        None => true,
        Some(named) => Some(named) == target,
    }
}

fn is_cached(cache: &Cache, artifact: &Artifact) -> bool {
    // A parsed manifest carries 64 hex characters, so a digest that fails here
    // is one nothing could have been cached under either way.
    ArtifactDigest::from_hex(&artifact.sha256).is_ok_and(|digest| cache.is_complete(&digest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(name: &str, target: Option<&str>, size: u64) -> Artifact {
        Artifact {
            name: name.to_string(),
            target: target.map(str::to_string),
            path: format!("{name}.tar.zst"),
            sha256: "ab".repeat(32),
            size,
            revision: "abc1234".to_string(),
            version: None,
        }
    }

    fn manifest(artifacts: Vec<Artifact>) -> Manifest {
        Manifest {
            schema: 1,
            version: 1,
            source_revision: "abc1234".to_string(),
            published: "2026-08-15T12:00:00Z".to_string(),
            artifacts,
        }
    }

    fn plan(manifest: &Manifest, target: Option<&str>) -> Plan {
        Plan::select("gen".to_string(), manifest, target, &Cache::new("/nowhere"))
    }

    #[test]
    fn an_untargeted_artifact_applies_to_every_consumer() {
        let artifact = artifact("db", None, 1);
        assert!(applies_to(&artifact, Some("linux-x86_64")));
        assert!(applies_to(&artifact, Some("windows-x86_64")));
        assert!(applies_to(&artifact, None));
    }

    #[test]
    fn a_targeted_artifact_applies_only_to_the_target_it_names() {
        let artifact = artifact("spu", Some("linux-x86_64"), 1);
        assert!(applies_to(&artifact, Some("linux-x86_64")));
        assert!(!applies_to(&artifact, Some("windows-x86_64")));
        assert!(!applies_to(&artifact, None));
    }

    #[test]
    fn selection_keeps_what_applies_and_drops_the_rest() {
        let manifest = manifest(vec![
            artifact("spu", Some("linux-x86_64"), 10),
            artifact("spu", Some("windows-x86_64"), 20),
            artifact("db", None, 30),
        ]);

        let selected = plan(&manifest, Some("linux-x86_64"));
        let names: Vec<&str> = selected
            .artifacts
            .iter()
            .map(|artifact| artifact.name.as_str())
            .collect();
        assert_eq!(names, ["spu", "db"]);
        assert_eq!(
            selected.artifacts[0].target.as_deref(),
            Some("linux-x86_64")
        );
        assert_eq!(selected.artifact_count(), 2);
        assert_eq!(selected.total_bytes, 40);

        let untargeted = plan(&manifest, None);
        assert_eq!(untargeted.artifact_count(), 1);
        assert_eq!(untargeted.artifacts[0].name, "db");
        assert_eq!(untargeted.total_bytes, 30);
    }

    #[test]
    fn a_target_nothing_matches_plans_nothing() {
        let manifest = manifest(vec![artifact("spu", Some("linux-x86_64"), 10)]);
        let selected = plan(&manifest, Some("macos-aarch64"));

        assert_eq!(selected.artifact_count(), 0);
        assert_eq!(selected.total_bytes, 0);
        assert_eq!(selected.bytes_to_fetch(), 0);
    }

    #[test]
    fn nothing_is_cached_when_the_cache_holds_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = manifest(vec![artifact("spu", None, 10)]);
        let selected = Plan::select("gen".to_string(), &manifest, None, &Cache::new(dir.path()));

        assert_eq!(selected.total_bytes, 10);
        assert_eq!(selected.cached_bytes, 0);
        assert_eq!(selected.bytes_to_fetch(), 10);
    }

    #[test]
    fn total_bytes_saturate_rather_than_overflowing() {
        let manifest = manifest(vec![
            artifact("a", None, u64::MAX),
            artifact("b", None, u64::MAX),
        ]);

        assert_eq!(plan(&manifest, None).total_bytes, u64::MAX);
    }
}
