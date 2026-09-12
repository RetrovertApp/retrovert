//! `init`: create a channel from nothing and sign its first generation.
//!
//! The online roles are always keyed here: their keys end up in a protected CI
//! environment, so generating them on the publisher's machine is the whole of
//! their custody story. The root is not, necessarily — see [`RootKeys`].

use std::collections::BTreeMap;
use std::path::PathBuf;

use jiff::Timestamp;
use retrovert_tuf::{
    Channel, KeyPair, MetaFile, PublicKey, RoleAuthorization, RoleName, Root, Signed, Snapshot,
    Targets, policy, published_names,
};

use crate::error::{Error, Result};
use crate::workspace::Workspace;

/// The version every role carries in a freshly initialized channel.
pub const INITIAL_VERSION: u64 = 1;

/// The roles whose keys a publish or a re-sign uses, and which therefore live
/// in the workspace's `keys/online/`.
pub const ONLINE_ROLES: [RoleName; 3] =
    [RoleName::Targets, RoleName::Snapshot, RoleName::Timestamp];

/// How a channel's `root` role is keyed.
///
/// The two variants are two custody stories, not two code paths for one story.
/// A generated root is as safe as the directory it was written to, which is
/// what a disposable test channel wants and what a production one must never
/// be. Held keys never touch this machine: the holders generate them
/// separately, only the public halves are brought together to describe the
/// role, and a threshold of the private halves is present just long enough to
/// sign the root this creates.
#[derive(Debug, Clone)]
pub enum RootKeys {
    /// One key generated here, authorized alone at threshold 1, and written to
    /// the workspace's offline key store.
    Generated(KeyPair),

    /// Keys held outside this workspace, a threshold of which is present to
    /// sign. Nothing is written to `keys/offline/`: there is nothing here to
    /// write.
    Held {
        /// Every key the `root` role will accept.
        authorized: Vec<PublicKey>,
        /// How many distinct ones a valid root signature set needs.
        threshold: u32,
        /// The private halves present at the ceremony. At least `threshold` of
        /// them, each one of `authorized`.
        signing: Vec<KeyPair>,
    },
}

impl RootKeys {
    /// How the `root` role is described in the root metadata.
    fn authorization(&self) -> RoleAuthorization {
        match self {
            Self::Generated(key) => RoleAuthorization::single(RoleName::Root, key.public()),
            Self::Held {
                authorized,
                threshold,
                ..
            } => RoleAuthorization {
                role: RoleName::Root,
                keys: authorized.clone(),
                threshold: *threshold,
            },
        }
    }

    /// The keys that sign the root being created.
    ///
    /// Checks that they can actually meet the threshold, and that every one of
    /// them is authorized. A root signed by too few keys, or by a key the role
    /// does not list, is a root no client accepts — and the ceremony that
    /// produced it is over by the time anyone finds out, so it is checked
    /// before a byte is written rather than after.
    fn signers(&self) -> Result<Vec<&KeyPair>> {
        match self {
            Self::Generated(key) => Ok(vec![key]),
            Self::Held {
                authorized,
                threshold,
                signing,
            } => {
                if signing.len() < *threshold as usize {
                    return Err(Error::NotEnoughRootSigners {
                        present: signing.len(),
                        threshold: *threshold,
                    });
                }

                let authorized_ids = authorized
                    .iter()
                    .map(PublicKey::key_id)
                    .collect::<retrovert_tuf::Result<Vec<_>>>()?;
                for key in signing {
                    let key_id = key.key_id()?;
                    if !authorized_ids.contains(&key_id) {
                        return Err(Error::UnauthorizedRootSigner(key_id));
                    }
                }
                Ok(signing.iter().collect())
            }
        }
    }

    /// The key to store in `keys/offline/`, when the channel has one.
    fn to_store(&self) -> Option<&KeyPair> {
        match self {
            Self::Generated(key) => Some(key),
            Self::Held { .. } => None,
        }
    }
}

/// The keys a channel is initialized with: one per online role, plus however
/// the root is keyed.
#[derive(Debug, Clone)]
pub struct KeySet {
    /// How `root` is keyed.
    pub root: RootKeys,
    /// Signs `targets`.
    pub targets: KeyPair,
    /// Signs `snapshot`.
    pub snapshot: KeyPair,
    /// Signs `timestamp`.
    pub timestamp: KeyPair,
}

impl KeySet {
    /// Generate four independent keys from the OS random source, root included.
    pub fn generate() -> Result<Self> {
        Ok(Self {
            root: RootKeys::Generated(KeyPair::generate()?),
            targets: KeyPair::generate()?,
            snapshot: KeyPair::generate()?,
            timestamp: KeyPair::generate()?,
        })
    }

    /// The key this set holds for `role`, if it holds one.
    ///
    /// `None` for `root` when the root keys are held elsewhere — which is the
    /// question a caller writing a key store is really asking.
    #[must_use]
    pub fn get(&self, role: RoleName) -> Option<&KeyPair> {
        match role {
            RoleName::Root => self.root.to_store(),
            RoleName::Targets => Some(&self.targets),
            RoleName::Snapshot => Some(&self.snapshot),
            RoleName::Timestamp => Some(&self.timestamp),
        }
    }

    /// How every role is authorized in the root metadata.
    fn authorizations(&self) -> Vec<RoleAuthorization> {
        let mut authorizations = vec![self.root.authorization()];
        authorizations.extend(ONLINE_ROLES.into_iter().map(|role| {
            RoleAuthorization::single(
                role,
                self.get(role)
                    .expect("an online role is always keyed here")
                    .public(),
            )
        }));
        authorizations
    }
}

/// What `init` wrote.
#[derive(Debug, Clone)]
pub struct InitReport {
    /// Metadata files written, in publication order.
    pub metadata: Vec<PathBuf>,
    /// Private-key files written.
    pub keys: Vec<PathBuf>,
    /// The `root` role's TUF key IDs, sorted — the fingerprints clients pin.
    pub root_key_ids: Vec<String>,
    /// How many of them a root signature set needs.
    pub root_threshold: u32,
}

/// Initialize `workspace` as a channel signed by `keys`, dated `now`.
///
/// Refuses a non-empty workspace unless `force`, so an existing channel's keys
/// cannot be silently replaced.
pub fn init(
    workspace: &Workspace,
    keys: &KeySet,
    now: Timestamp,
    force: bool,
) -> Result<InitReport> {
    if !force && !workspace.is_empty()? {
        return Err(Error::NotEmpty(workspace.path().to_path_buf()));
    }

    // The root payload is built and its signers checked before anything is
    // written. A root whose threshold no key set could meet, or which is signed
    // by a key it never authorized, must not leave a half-made channel and a
    // set of private keys behind it — a ceremony is over by the time anyone
    // reads the error.
    let root_metadata = Root::new(
        INITIAL_VERSION,
        policy::expires(RoleName::Root, now)?,
        &keys.authorizations(),
    )?;
    let root_signers = keys.root.signers()?;
    let root_role = root_metadata.roles[RoleName::Root.as_str()].clone();

    let store = workspace.keys();
    store.create_dirs()?;
    let mut written_keys = Vec::new();
    for role in RoleName::ALL {
        if let Some(key) = keys.get(role) {
            store.write(role, key)?;
            written_keys.push(store.key_path(role));
        }
    }

    let channel = workspace.channel();
    // A forced re-init replaces the channel wholesale. Clear the published
    // directories first: metadata and targets from the previous channel are
    // unreferenced by the new chain but would otherwise still be uploaded and
    // fetchable by direct URL.
    if force {
        for dir in [channel.metadata_dir(), channel.targets_dir()] {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(Error::io(dir, e)),
            }
        }
    }
    channel.create_dirs()?;

    // Signing order is the publication order: a role is signed only after
    // everything it pins, so timestamp lands last and commits the set.
    let root = Signed::new(root_metadata, &root_signers)?.to_json()?;

    let targets = Signed::new(
        Targets::new(
            INITIAL_VERSION,
            policy::expires(RoleName::Targets, now)?,
            BTreeMap::new(),
        ),
        &[&keys.targets],
    )?
    .to_json()?;

    let snapshot = Signed::new(
        Snapshot::new(
            INITIAL_VERSION,
            policy::expires(RoleName::Snapshot, now)?,
            BTreeMap::from([(
                RoleName::Targets.file_name(),
                MetaFile::pinning(INITIAL_VERSION, &targets),
            )]),
        ),
        &[&keys.snapshot],
    )?
    .to_json()?;

    let timestamp = Signed::new(
        retrovert_tuf::Timestamp::new(
            INITIAL_VERSION,
            policy::expires(RoleName::Timestamp, now)?,
            MetaFile::pinning(INITIAL_VERSION, &snapshot),
        ),
        &[&keys.timestamp],
    )?
    .to_json()?;

    let written = [
        (RoleName::Root, root),
        (RoleName::Targets, targets),
        (RoleName::Snapshot, snapshot),
        (RoleName::Timestamp, timestamp),
    ]
    .into_iter()
    .map(|(role, bytes)| write_role(&channel, role, &bytes))
    .collect::<Result<Vec<_>>>()?
    .concat();

    Ok(InitReport {
        metadata: written,
        keys: written_keys,
        root_key_ids: root_role.keyids,
        root_threshold: root_role.threshold,
    })
}

fn write_role(channel: &Channel, role: RoleName, bytes: &[u8]) -> Result<Vec<PathBuf>> {
    published_names(role, INITIAL_VERSION)
        .into_iter()
        .map(|name| {
            channel.write_metadata(&name, bytes)?;
            Ok(channel.metadata_dir().join(name))
        })
        .collect()
}
