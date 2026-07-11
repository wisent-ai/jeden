use super::cas::{build_snapshot, materialize_snapshot, CasError, Digest, LocalCas, UploadStatus};
use super::coordinator::Coordinator;
use super::protocol::{
    negotiate_version, AttemptPhase, CommitRequest, Job, JobPhase, PlacementConstraints,
    ProtocolError, ProtocolVersion, Resources, VersionRange, WorkerDescriptor, WorkerEvent,
    WorkerHello,
};
use super::worker::{LoopbackTransport, WorkerRuntime};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "jeden-distributed-{name}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn set(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn worker(worker_id: &str, residencies: &[&str]) -> WorkerHello {
    WorkerHello {
        worker_id: worker_id.into(),
        versions: VersionRange::default(),
        descriptor: WorkerDescriptor {
            os: "linux".into(),
            arch: "arm64".into(),
            capabilities: set(&["execute"]),
            sandbox_profiles: set(&["strict"]),
            trust_zones: set(&["trusted"]),
            residencies: set(residencies),
            resources: Resources {
                cpu_millis: 2_000,
                memory_bytes: 1 << 30,
                disk_bytes: 1 << 30,
            },
            cas_objects: BTreeSet::new(),
            labels: BTreeMap::new(),
            max_parallel: 1,
        },
        incarnation: 1,
    }
}

fn job(id: &str, input_root: Digest) -> Job {
    Job {
        id: id.into(),
        input_root,
        constraints: PlacementConstraints::default(),
        payload: b"payload".to_vec(),
        created_at: 1,
    }
}

fn empty_snapshot(cas: &LocalCas, root: &Path) -> Digest {
    let input = root.join("empty-input");
    fs::create_dir(&input).expect("create input");
    build_snapshot(cas, input).expect("build empty input snapshot")
}

#[test]
fn protocol_v1_accepts_compatible_ranges_and_rejects_invalid_or_disjoint_ranges() {
    let compatible = [
        VersionRange {
            minimum: ProtocolVersion { major: 1, minor: 0 },
            maximum: ProtocolVersion { major: 1, minor: 0 },
        },
        VersionRange {
            minimum: ProtocolVersion { major: 0, minor: 9 },
            maximum: ProtocolVersion { major: 1, minor: 7 },
        },
    ];
    for range in compatible {
        assert_eq!(negotiate_version(&range), Ok(ProtocolVersion::V1));
    }

    let unsupported = VersionRange {
        minimum: ProtocolVersion { major: 2, minor: 0 },
        maximum: ProtocolVersion { major: 2, minor: 4 },
    };
    assert!(matches!(
        negotiate_version(&unsupported),
        Err(ProtocolError::UnsupportedVersion { .. })
    ));

    let inverted = VersionRange {
        minimum: ProtocolVersion { major: 1, minor: 1 },
        maximum: ProtocolVersion { major: 1, minor: 0 },
    };
    assert!(
        matches!(negotiate_version(&inverted), Err(ProtocolError::Invalid(message)) if message.contains("inverted"))
    );
}

#[test]
fn residency_is_a_hard_placement_constraint() {
    let fixture = TempDirectory::new("residency");
    let coordinator = Coordinator::open(fixture.path(), 500).unwrap();
    let input = empty_snapshot(&coordinator.cas, fixture.path());
    coordinator
        .register_worker(worker("worker-eu", &["eu"]), 10)
        .unwrap();
    let mut constrained = job("residency-job", input);
    constrained.constraints.residency = Some("us".into());
    coordinator.submit(constrained).unwrap();

    let error = coordinator.assign("residency-job", 20).unwrap_err();
    assert!(
        matches!(error, ProtocolError::NoPlacement(message) if message.contains("wrong residency us"))
    );
    assert_eq!(
        coordinator.job("residency-job").unwrap().phase,
        JobPhase::Pending
    );
}

#[test]
fn expired_lease_is_reassigned_with_a_new_fence_and_rejects_the_stale_commit() {
    let fixture = TempDirectory::new("lease-fence");
    let coordinator = Coordinator::open(fixture.path(), 100).unwrap();
    let input = empty_snapshot(&coordinator.cas, fixture.path());
    coordinator
        .register_worker(worker("worker-a", &["eu"]), 0)
        .unwrap();
    coordinator.submit(job("leased-job", input)).unwrap();
    let first = coordinator.assign("leased-job", 10).unwrap();
    coordinator
        .acknowledge(
            "worker-a",
            "leased-job",
            first.attempt,
            first.fencing_token,
            11,
        )
        .unwrap();

    assert_eq!(
        coordinator.expire_leases(first.lease_expires_at).unwrap(),
        vec!["leased-job"]
    );
    let second = coordinator
        .assign("leased-job", first.lease_expires_at + 1)
        .unwrap();
    assert_eq!(second.attempt, first.attempt + 1);
    assert!(second.fencing_token > first.fencing_token);

    let output_root = coordinator.cas.put(b"output").unwrap();
    let stale = coordinator
        .commit(
            "worker-a",
            CommitRequest {
                job_id: "leased-job".into(),
                attempt: first.attempt,
                fencing_token: first.fencing_token,
                output_root,
                result: b"stale".to_vec(),
            },
            first.lease_expires_at + 2,
        )
        .unwrap_err();
    assert_eq!(
        stale,
        ProtocolError::StaleFence {
            expected: second.fencing_token,
            actual: first.fencing_token
        }
    );
    assert_eq!(
        coordinator.job("leased-job").unwrap().phase,
        JobPhase::Assigned
    );
}

#[test]
fn coordinator_restart_adopts_a_live_attempt_without_changing_its_identity() {
    let fixture = TempDirectory::new("restart-adopt");
    let coordinator = Coordinator::open(fixture.path(), 1_000).unwrap();
    let input = empty_snapshot(&coordinator.cas, fixture.path());
    let hello = worker("worker-a", &["eu"]);
    let initial_epoch = coordinator
        .register_worker(hello.clone(), 10)
        .unwrap()
        .coordinator_epoch;
    coordinator.submit(job("adopted-job", input)).unwrap();
    let assigned = coordinator.assign("adopted-job", 20).unwrap();
    coordinator
        .acknowledge(
            "worker-a",
            "adopted-job",
            assigned.attempt,
            assigned.fencing_token,
            21,
        )
        .unwrap();
    drop(coordinator);

    let restarted = Coordinator::open(fixture.path(), 1_000).unwrap();
    let new_epoch = restarted
        .register_worker(hello, 30)
        .unwrap()
        .coordinator_epoch;
    assert!(new_epoch > initial_epoch);
    let adopted = restarted
        .adopt(
            "worker-a",
            "adopted-job",
            assigned.attempt,
            assigned.fencing_token,
            31,
        )
        .unwrap();
    assert_eq!(
        (adopted.attempt, adopted.fencing_token),
        (assigned.attempt, assigned.fencing_token)
    );
    assert!(adopted.lease_expires_at > assigned.lease_expires_at);
    assert_eq!(
        restarted.job("adopted-job").unwrap().phase,
        JobPhase::Running
    );
}

#[test]
fn worker_event_retries_are_deduplicated_and_sequence_gaps_are_rejected() {
    let fixture = TempDirectory::new("event-replay");
    let coordinator = Coordinator::open(fixture.path(), 1_000).unwrap();
    let input = empty_snapshot(&coordinator.cas, fixture.path());
    coordinator
        .register_worker(worker("worker-a", &["eu"]), 0)
        .unwrap();
    coordinator.submit(job("event-job", input)).unwrap();
    let offer = coordinator.assign("event-job", 1).unwrap();
    coordinator
        .acknowledge(
            "worker-a",
            "event-job",
            offer.attempt,
            offer.fencing_token,
            2,
        )
        .unwrap();
    let event = WorkerEvent {
        job_id: "event-job".into(),
        attempt: offer.attempt,
        fencing_token: offer.fencing_token,
        sequence: 1,
        phase: AttemptPhase::Running,
        detail: "started".into(),
    };

    assert!(coordinator
        .record_event("worker-a", event.clone(), 3)
        .unwrap());
    assert!(!coordinator
        .record_event("worker-a", event.clone(), 4)
        .unwrap());
    let gap = WorkerEvent {
        sequence: 3,
        phase: AttemptPhase::Uploading,
        detail: "upload".into(),
        ..event.clone()
    };
    assert!(
        matches!(coordinator.record_event("worker-a", gap, 5), Err(ProtocolError::Conflict(message)) if message.contains("expected 2, got 3"))
    );
    assert_eq!(
        coordinator
            .replay_events("event-job", offer.attempt, 0)
            .unwrap(),
        vec![event]
    );
    assert!(coordinator
        .replay_events("event-job", offer.attempt, 1)
        .unwrap()
        .is_empty());
}

#[test]
fn cancellation_is_terminal_before_assignment_and_requires_worker_confirmation_when_active() {
    let fixture = TempDirectory::new("cancellation");
    let coordinator = Coordinator::open(fixture.path(), 1_000).unwrap();
    let input = empty_snapshot(&coordinator.cas, fixture.path());
    coordinator
        .register_worker(worker("worker-a", &["eu"]), 0)
        .unwrap();

    coordinator.submit(job("pending-cancel", input)).unwrap();
    assert!(coordinator.cancel("pending-cancel", 1).unwrap());
    assert_eq!(
        coordinator.job("pending-cancel").unwrap().phase,
        JobPhase::Cancelled
    );
    assert!(matches!(
        coordinator.assign("pending-cancel", 2),
        Err(ProtocolError::Conflict(_))
    ));

    coordinator.submit(job("active-cancel", input)).unwrap();
    let offer = coordinator.assign("active-cancel", 10).unwrap();
    coordinator
        .acknowledge(
            "worker-a",
            "active-cancel",
            offer.attempt,
            offer.fencing_token,
            11,
        )
        .unwrap();
    assert!(coordinator.cancel("active-cancel", 12).unwrap());
    assert_eq!(
        coordinator.job("active-cancel").unwrap().phase,
        JobPhase::Cancelling
    );
    let output_root = coordinator.cas.put(b"too late").unwrap();
    let commit = CommitRequest {
        job_id: "active-cancel".into(),
        attempt: offer.attempt,
        fencing_token: offer.fencing_token,
        output_root,
        result: Vec::new(),
    };
    assert!(matches!(
        coordinator.commit("worker-a", commit, 13),
        Err(ProtocolError::Cancelled(_))
    ));
    coordinator
        .confirm_cancelled(
            "worker-a",
            "active-cancel",
            offer.attempt,
            offer.fencing_token,
            14,
        )
        .unwrap();
    assert_eq!(
        coordinator.job("active-cancel").unwrap().phase,
        JobPhase::Cancelled
    );
}

#[test]
fn local_cas_detects_corruption_and_resumes_upload_at_the_durable_offset() {
    let fixture = TempDirectory::new("cas");
    let cas = LocalCas::open(fixture.path().join("cas")).unwrap();
    let bytes = b"resumable object bytes";
    let digest = Digest::of(bytes);
    assert_eq!(
        cas.upload_chunk(digest, 0, &bytes[..9], false).unwrap(),
        UploadStatus::Incomplete { offset: 9 }
    );
    drop(cas);

    let cas = LocalCas::open(fixture.path().join("cas")).unwrap();
    assert_eq!(cas.upload_offset(digest).unwrap(), 9);
    assert!(matches!(
        cas.upload_chunk(digest, 8, &bytes[9..], true),
        Err(CasError::InvalidOffset {
            expected: 9,
            actual: 8
        })
    ));
    assert_eq!(
        cas.upload_chunk(digest, 9, &bytes[9..], true).unwrap(),
        UploadStatus::Complete { digest }
    );
    assert_eq!(cas.get(digest).unwrap(), bytes);

    let encoded = digest.to_string();
    let object = cas
        .root()
        .join("objects")
        .join(&encoded[..2])
        .join(&encoded[2..]);
    let mut permissions = fs::metadata(&object).unwrap().permissions();
    permissions.set_readonly(false);
    fs::set_permissions(&object, permissions).unwrap();
    fs::write(&object, b"corrupt bytes").unwrap();
    assert!(
        matches!(cas.get(digest), Err(CasError::CorruptObject { expected, .. }) if expected == digest)
    );
    assert!(
        matches!(cas.contains(digest), Err(CasError::CorruptObject { expected, .. }) if expected == digest)
    );
}

#[cfg(unix)]
#[test]
fn snapshots_are_deterministic_and_reject_symlinks_and_case_fold_collisions() {
    use std::os::unix::fs::symlink;

    let fixture = TempDirectory::new("snapshots");
    let cas = LocalCas::open(fixture.path().join("cas")).unwrap();
    let first = fixture.path().join("first");
    let second = fixture.path().join("second");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    fs::write(first.join("z.txt"), b"last").unwrap();
    fs::write(first.join("a.txt"), b"first").unwrap();
    fs::write(second.join("a.txt"), b"first").unwrap();
    fs::write(second.join("z.txt"), b"last").unwrap();
    assert_eq!(
        build_snapshot(&cas, &first).unwrap(),
        build_snapshot(&cas, &second).unwrap()
    );

    let links = fixture.path().join("links");
    fs::create_dir(&links).unwrap();
    fs::write(links.join("target"), b"data").unwrap();
    symlink("target", links.join("alias")).unwrap();
    assert!(
        matches!(build_snapshot(&cas, &links), Err(CasError::UnsupportedEntry(path)) if path.ends_with("alias"))
    );

    let file_digest = cas.put(b"collision content").unwrap();
    let collision_tree = serde_json::json!({
        "schema": "jeden.merkle-tree.v1",
        "entries": [
            { "name": "A", "kind": "file", "digest": file_digest },
            { "name": "a", "kind": "file", "digest": file_digest }
        ]
    });
    let collision_root = cas
        .put(&serde_json::to_vec(&collision_tree).unwrap())
        .unwrap();
    assert!(matches!(
        materialize_snapshot(&cas, collision_root, fixture.path().join("collision-output")),
        Err(CasError::CaseCollision { first, second, .. }) if first == "A" && second == "a"
    ));
}

fn run_loopback(root: &Path, serialized: bool) -> super::protocol::JobOutcome {
    let coordinator = Coordinator::open(root.join("coordinator"), 1_000).unwrap();
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("input.txt"), b"input bytes").unwrap();
    let input_root = build_snapshot(&coordinator.cas, &source).unwrap();
    coordinator.submit(job("loopback-job", input_root)).unwrap();
    let runtime = WorkerRuntime::open(
        root.join("worker"),
        worker("loopback-worker", &["eu"]),
        coordinator.cas.clone(),
        Arc::new(
            |input: &Path, output: &Path, payload: &[u8], _cancelled: &dyn Fn() -> bool| {
                let input_bytes =
                    fs::read(input.join("input.txt")).map_err(|error| error.to_string())?;
                fs::write(
                    output.join("result.txt"),
                    [input_bytes.as_slice(), payload].concat(),
                )
                .map_err(|error| error.to_string())?;
                Ok(b"executor-result".to_vec())
            },
        ),
    )
    .unwrap();
    let transport = if serialized {
        LoopbackTransport::remote(runtime)
    } else {
        LoopbackTransport::local(runtime)
    };
    transport.run(&coordinator, "loopback-job", 10).unwrap()
}

#[test]
fn serialized_loopback_has_the_same_outcome_as_in_process_transport() {
    let local = TempDirectory::new("loopback-local");
    let remote = TempDirectory::new("loopback-remote");
    let local_outcome = run_loopback(local.path(), false);
    let remote_outcome = run_loopback(remote.path(), true);

    assert_eq!(remote_outcome, local_outcome);
    assert_eq!(local_outcome.result, b"executor-result");
}
