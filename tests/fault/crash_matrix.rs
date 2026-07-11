#[path = "../../rust/test_support/mod.rs"]
mod test_support;

use test_support::failpoints::Failpoint;
use test_support::fake_disk::FakeDisk;
use test_support::fake_network::FakeNetwork;
use test_support::invariants::ReliabilityState;

#[test]
fn atomic_replace_is_old_or_new_at_every_crash_boundary() {
    for point in [
        Failpoint::BeforeWrite,
        Failpoint::AfterWriteBeforeSync,
        Failpoint::AfterSyncBeforeRename,
        Failpoint::AfterRename,
    ] {
        let mut disk = FakeDisk::bounded(1024);
        disk.atomic_replace("ledger", b"old").expect("seed");
        disk.failpoints().arm(point, 1);
        assert_eq!(disk.atomic_replace("ledger", b"new"), Err(point));
        assert!(matches!(disk.read("ledger"), Some(b"old") | Some(b"new")));
    }
}

#[test]
fn private_export_is_default_off_and_each_transport_failure_is_observable() {
    let network = FakeNetwork::default();
    assert_eq!(network.send_allowlisted(1), Err(Failpoint::NetworkConnect));
    assert_eq!(network.requests(), 0);
    network.set_opt_in(true);
    for point in [
        Failpoint::NetworkConnect,
        Failpoint::NetworkFirstByte,
        Failpoint::NetworkIdle,
    ] {
        network.failpoints().arm(point, 1);
        assert_eq!(network.send_allowlisted(1), Err(point));
    }
}

#[test]
fn invariant_oracle_rejects_each_release_blocking_failure() {
    let mut good = ReliabilityState {
        queue_limit: 4,
        ..ReliabilityState::default()
    };
    good.ledger_parents.insert("root".into(), None);
    good.terminal("root");
    good.cancellation_millis.push(5);
    assert!(good.assert_clean(&["secret-canary"], 5_000).is_ok());

    let mut orphan = ReliabilityState {
        queue_limit: 4,
        ..ReliabilityState::default()
    };
    orphan.live_processes.insert(7);
    assert!(orphan
        .assert_clean(&[], 5_000)
        .unwrap_err()
        .contains("orphan"));

    let mut duplicate = ReliabilityState {
        queue_limit: 4,
        ..ReliabilityState::default()
    };
    duplicate.terminal("op");
    duplicate.terminal("op");
    assert!(duplicate
        .assert_clean(&[], 5_000)
        .unwrap_err()
        .contains("terminal"));

    let mut secret = ReliabilityState {
        queue_limit: 4,
        ..ReliabilityState::default()
    };
    secret
        .durable_bytes
        .extend_from_slice(b"prefix-secret-canary-suffix");
    assert!(secret
        .assert_clean(&["secret-canary"], 5_000)
        .unwrap_err()
        .contains("secret"));

    let mut cycle = ReliabilityState {
        queue_limit: 4,
        ..ReliabilityState::default()
    };
    cycle.ledger_parents.insert("a".into(), Some("b".into()));
    cycle.ledger_parents.insert("b".into(), Some("a".into()));
    assert!(cycle
        .assert_clean(&[], 5_000)
        .unwrap_err()
        .contains("cycle"));
}
