//! A root held by several people: what `init` writes for it, what it refuses,
//! and that the real client library accepts the result.
//!
//! The threshold is the point of the whole ceremony, so every assertion here
//! goes through `sigstore-tuf` rather than through this crate's own view of
//! what it wrote. A root that reads as 2-of-3 and verifies as something else is
//! exactly the failure a publisher would never notice on its own.

mod common;

use std::process::Command;

use common::{now, read, refresh_with_sigstore_tuf};
use retrovert_publish::{Error, KeySet, RootKeys, Workspace, init};
use retrovert_tuf::{KeyPair, PublicKey, RoleName};
use sigstore_tuf::Metadata;
use tempfile::TempDir;

/// The three keys the holders bring, fixed so failures are reproducible.
fn holders() -> [KeyPair; 3] {
    [
        KeyPair::from_seed(&[11u8; 32]),
        KeyPair::from_seed(&[12u8; 32]),
        KeyPair::from_seed(&[13u8; 32]),
    ]
}

fn authorized(holders: &[KeyPair; 3]) -> Vec<PublicKey> {
    holders.iter().map(KeyPair::public).collect()
}

/// A key set whose online roles are fixed and whose root is held by `holders`,
/// signed by whichever of them `signing` names by index.
fn ceremony_keys(threshold: u32, signing: &[usize]) -> KeySet {
    let holders = holders();
    KeySet {
        root: RootKeys::Held {
            authorized: authorized(&holders),
            threshold,
            signing: signing.iter().map(|i| holders[*i].clone()).collect(),
        },
        targets: KeyPair::from_seed(&[2u8; 32]),
        snapshot: KeyPair::from_seed(&[3u8; 32]),
        timestamp: KeyPair::from_seed(&[4u8; 32]),
    }
}

fn workspace() -> (TempDir, Workspace) {
    let dir = TempDir::new().unwrap();
    let workspace = Workspace::new(dir.path().join("channel"));
    (dir, workspace)
}

#[test]
fn a_two_of_three_root_is_written_and_the_client_accepts_it() {
    let (_dir, workspace) = workspace();
    let report = init(&workspace, &ceremony_keys(2, &[0, 1]), now(), false).unwrap();

    assert_eq!(report.root_threshold, 2);
    assert_eq!(report.root_key_ids.len(), 3);

    let root = Metadata::<sigstore_tuf::Root>::from_slice(&read(&workspace, "root.json"))
        .unwrap()
        .signed;
    let entry = root.role("root").expect("root is authorized");
    assert_eq!(entry.threshold, 2);
    assert_eq!(entry.keyids.len(), 3);
    // Two of the three signed; the third holder was not in the room.
    assert_eq!(
        Metadata::<sigstore_tuf::Root>::from_slice(&read(&workspace, "root.json"))
            .unwrap()
            .signatures
            .len(),
        2
    );

    refresh_with_sigstore_tuf(&workspace, now()).expect("a 2-of-3 root must verify");
}

/// The reason to hold three keys rather than two: any pair of them works, so
/// losing a holder costs a re-sign rather than a re-root.
#[test]
fn any_pair_of_the_three_holders_produces_a_root_the_client_accepts() {
    for pair in [[0, 1], [0, 2], [1, 2]] {
        let (_dir, workspace) = workspace();
        init(&workspace, &ceremony_keys(2, &pair), now(), false).unwrap();
        refresh_with_sigstore_tuf(&workspace, now())
            .unwrap_or_else(|e| panic!("holders {pair:?} produced a root that fails: {e}"));
    }
}

/// The online roles stay single-key: a re-sign job holds one key per role and
/// its threshold is not what the ceremony is about.
#[test]
fn the_online_roles_are_unchanged_by_a_held_root() {
    let (_dir, workspace) = workspace();
    init(&workspace, &ceremony_keys(2, &[0, 1]), now(), false).unwrap();

    let root = Metadata::<sigstore_tuf::Root>::from_slice(&read(&workspace, "root.json"))
        .unwrap()
        .signed;
    for role in ["targets", "snapshot", "timestamp"] {
        let entry = root.role(role).expect("role is authorized");
        assert_eq!(entry.threshold, 1, "{role}");
        assert_eq!(entry.keyids.len(), 1, "{role}");
    }
    // Three online keys plus three root holders, each key distinct.
    assert_eq!(root.keys.len(), 6);
}

#[test]
fn a_held_root_key_is_never_written_to_the_workspace() {
    let (_dir, workspace) = workspace();
    let report = init(&workspace, &ceremony_keys(2, &[0, 1]), now(), false).unwrap();

    let store = workspace.keys();
    assert!(
        !store.key_path(RoleName::Root).exists(),
        "a root key that lives on someone's stick must not be copied here"
    );
    assert_eq!(
        std::fs::read_dir(store.offline_dir()).unwrap().count(),
        0,
        "the offline directory exists but holds nothing"
    );
    assert_eq!(
        report.keys.len(),
        3,
        "only the three online keys are stored"
    );
}

#[test]
fn a_root_signed_below_its_own_threshold_is_refused_before_anything_is_written() {
    let (_dir, workspace) = workspace();
    let error = init(&workspace, &ceremony_keys(2, &[0]), now(), false).unwrap_err();

    assert!(matches!(
        error,
        Error::NotEnoughRootSigners {
            present: 1,
            threshold: 2
        }
    ));
    assert!(
        workspace.is_empty().unwrap(),
        "a refused ceremony must leave no keys and no channel behind"
    );
}

#[test]
fn a_signer_the_root_does_not_authorize_is_refused() {
    let (_dir, workspace) = workspace();
    let holders = holders();
    let outsider = KeyPair::from_seed(&[99u8; 32]);
    let keys = KeySet {
        root: RootKeys::Held {
            authorized: authorized(&holders),
            threshold: 2,
            signing: vec![holders[0].clone(), outsider.clone()],
        },
        targets: KeyPair::from_seed(&[2u8; 32]),
        snapshot: KeyPair::from_seed(&[3u8; 32]),
        timestamp: KeyPair::from_seed(&[4u8; 32]),
    };

    let error = init(&workspace, &keys, now(), false).unwrap_err();

    assert!(
        matches!(error, Error::UnauthorizedRootSigner(id) if id == outsider.key_id().unwrap()),
        "the one signature that would not have counted has to be named"
    );
    assert!(workspace.is_empty().unwrap());
}

#[test]
fn a_threshold_no_holder_set_could_meet_is_refused() {
    let (_dir, workspace) = workspace();
    let error = init(&workspace, &ceremony_keys(4, &[0, 1, 2]), now(), false).unwrap_err();

    assert!(matches!(
        error,
        Error::Tuf(retrovert_tuf::Error::UnsatisfiableThreshold {
            role: RoleName::Root,
            threshold: 4,
            keys: 3
        })
    ));
}

/// The ceremony as it is actually run: each holder generates a key with
/// `keygen`, the public halves are brought together, and `init` writes the
/// channel with two of the private halves in the room.
#[test]
fn the_cli_runs_the_ceremony_end_to_end() {
    let dir = TempDir::new().unwrap();
    let cli = || Command::new(env!("CARGO_BIN_EXE_retrovert-publish"));
    let run = |command: &mut Command| {
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };

    let private = |holder: &str| dir.path().join(format!("{holder}.pem"));
    let public = |holder: &str| dir.path().join(format!("{holder}.pub.json"));
    for holder in ["alice", "bob", "carol"] {
        let stdout = run(cli().arg("keygen").arg(private(holder)));
        assert!(stdout.contains("key id:"), "keygen reports the fingerprint");
        assert!(public(holder).exists(), "the travelling half is written");
    }

    let workspace = Workspace::new(dir.path().join("channel"));
    let stdout = run(cli()
        .arg("init")
        .arg(workspace.path())
        .arg("--root-threshold")
        .arg("2")
        .args(["--root-key", public("alice").to_str().unwrap()])
        .args(["--root-key", public("bob").to_str().unwrap()])
        .args(["--root-key", public("carol").to_str().unwrap()])
        // Carol's stick stayed at home.
        .args(["--root-sign-with", private("alice").to_str().unwrap()])
        .args(["--root-sign-with", private("bob").to_str().unwrap()]));

    assert!(stdout.contains("root:     2 of 3 key(s)"), "{stdout}");
    refresh_with_sigstore_tuf(&workspace, jiff::Timestamp::now())
        .expect("the ceremony's channel must verify as a client sees it");
}

/// Naming keys without a threshold must not quietly produce a 1-of-3.
#[test]
fn the_cli_defaults_the_threshold_to_every_authorized_key() {
    let dir = TempDir::new().unwrap();
    let holder = |name: &str| dir.path().join(name);
    for name in ["a", "b"] {
        assert!(
            Command::new(env!("CARGO_BIN_EXE_retrovert-publish"))
                .arg("keygen")
                .arg(holder(&format!("{name}.pem")))
                .status()
                .unwrap()
                .success()
        );
    }

    let workspace = Workspace::new(dir.path().join("channel"));
    let output = Command::new(env!("CARGO_BIN_EXE_retrovert-publish"))
        .arg("init")
        .arg(workspace.path())
        .args(["--root-key", holder("a.pub.json").to_str().unwrap()])
        .args(["--root-key", holder("b.pub.json").to_str().unwrap()])
        .args(["--root-sign-with", holder("a.pem").to_str().unwrap()])
        .args(["--root-sign-with", holder("b.pem").to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        String::from_utf8_lossy(&output.stdout).contains("root:     2 of 2 key(s)"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

/// With no root flags the channel keys itself, which is what `dev` wants and
/// what the ceremony exists to avoid for `stable`.
#[test]
fn the_cli_still_generates_a_disposable_root_when_no_holders_are_named() {
    let dir = TempDir::new().unwrap();
    let workspace = Workspace::new(dir.path().join("channel"));

    let output = Command::new(env!("CARGO_BIN_EXE_retrovert-publish"))
        .arg("init")
        .arg(workspace.path())
        .output()
        .unwrap();

    assert!(String::from_utf8_lossy(&output.stdout).contains("root:     1 of 1 key(s)"));
    assert!(workspace.keys().key_path(RoleName::Root).exists());
    refresh_with_sigstore_tuf(&workspace, jiff::Timestamp::now()).unwrap();
}

#[cfg(unix)]
#[test]
fn a_generated_holder_key_is_readable_only_by_its_owner() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let private = dir.path().join("holder.pem");
    Command::new(env!("CARGO_BIN_EXE_retrovert-publish"))
        .arg("keygen")
        .arg(&private)
        .status()
        .unwrap();

    assert_eq!(
        std::fs::metadata(&private).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

/// `check` is the only thing a ceremony can run: no network, no published
/// generation, and the same client library a device runs.
mod check {
    use super::{ceremony_keys, workspace};
    use crate::common::{now, read};

    use retrovert_publish::{check, init};
    use retrovert_tuf::RoleName;
    use std::process::Command;

    #[test]
    fn it_reports_the_threshold_the_ceremony_produced() {
        let (_dir, workspace) = workspace();
        init(&workspace, &ceremony_keys(2, &[0, 1]), now(), false).unwrap();

        let chain = check(&workspace, now()).unwrap();

        assert_eq!(chain.root_threshold, 2);
        assert_eq!(chain.root_key_ids.len(), 3);
        assert_eq!(chain.root_signatures, 2);
        assert_eq!(
            chain.roles.iter().map(|r| r.role).collect::<Vec<_>>(),
            RoleName::ALL,
            "a refreshed chain establishes all four roles"
        );
        for role in &chain.roles {
            assert_eq!(role.version, 1);
        }
    }

    /// The check has to fail on a root that does not meet its own bar, or it
    /// is not a check. A signature is replaced with one made over other bytes,
    /// which is what a signer using the wrong key would leave behind.
    #[test]
    fn it_refuses_a_root_that_no_longer_meets_its_threshold() {
        let (_dir, workspace) = workspace();
        init(&workspace, &ceremony_keys(2, &[0, 1]), now(), false).unwrap();

        let path = workspace.channel().metadata_dir().join("root.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&read(&workspace, "root.json")).unwrap();
        // One of the two signatures is now noise; a 2-of-3 with one good
        // signature must not verify.
        value["signatures"][0]["sig"] = "00".repeat(64).into();
        std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        assert!(
            check(&workspace, now()).is_err(),
            "one valid signature must not satisfy a threshold of two"
        );
    }

    #[test]
    fn the_cli_reports_the_chain_without_touching_a_network() {
        let (_dir, workspace) = workspace();
        // The binary reads the real clock, so the channel is dated by it too:
        // `check` enforces expiry like any client, and a channel dated to the
        // fixed test instant is long past its 90 days.
        init(
            &workspace,
            &ceremony_keys(2, &[0, 1]),
            jiff::Timestamp::now(),
            false,
        )
        .unwrap();

        let output = Command::new(env!("CARGO_BIN_EXE_retrovert-publish"))
            .arg("check")
            .arg(workspace.path())
            .output()
            .unwrap();

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout.contains("root:     2 of 3 key(s), 2 signature(s)"),
            "{stdout}"
        );
        assert!(
            stdout.contains("verified: the chain refreshes against its own root"),
            "{stdout}"
        );
    }
}
