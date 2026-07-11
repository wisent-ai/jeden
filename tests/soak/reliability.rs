#[path = "../../rust/test_support/mod.rs"]
mod test_support;

use std::time::{Duration, Instant};

use test_support::fake_disk::FakeDisk;
use test_support::fault_clock::FaultClock;
use test_support::invariants::ReliabilityState;

const SECRET_CANARY: &str = "JEDEN_SOAK_SECRET_CANARY_7f31";

#[test]
fn mixed_operations_preserve_release_invariants() {
    let target_operations = env_u64("JEDEN_SOAK_OPERATIONS", 1_000);
    let real_seconds = env_u64("JEDEN_SOAK_REAL_SECONDS", 0);
    assert!(
        target_operations >= 1_000,
        "soak profile must execute at least 1000 operations"
    );

    let clock = FaultClock::at(1_000);
    let started = Instant::now();
    let mut disk = FakeDisk::bounded(64 * 1024);
    let mut state = ReliabilityState {
        queue_limit: 64,
        ..ReliabilityState::default()
    };
    state.ledger_parents.insert("root".into(), None);
    state.terminal("root");
    let mut completed = 0_u64;
    let mut random = 0x5eed_cafe_f00d_u64;

    while completed < target_operations || started.elapsed() < Duration::from_secs(real_seconds) {
        if completed >= target_operations && real_seconds > 0 {
            std::thread::sleep(Duration::from_secs(1));
        }
        random = random
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let operation = format!("operation-{completed}");
        let outbox = format!("outbox-{completed}");
        let lease = format!("lease-{completed}");
        let process = completed + 10;

        state
            .ledger_parents
            .insert(operation.clone(), Some("root".into()));
        state.pending_outbox.insert(outbox.clone());
        state.live_leases.insert(lease.clone());
        state.live_processes.insert(process);
        state.observe_queue((random as usize % state.queue_limit) + 1);

        // Durable data contains only typed counters/pseudonyms, never model/tool/secret payloads.
        let durable = format!("{{\"sequence\":{completed},\"operation\":\"pid_{random:016x}\"}}");
        disk.atomic_replace("telemetry-spool", durable.as_bytes())
            .expect("bounded atomic spool");
        clock.advance(Duration::from_millis((random % 7) + 1));

        state.pending_outbox.remove(&outbox);
        state.live_leases.remove(&lease);
        state.live_processes.remove(&process);
        state.cancellation_millis.push(random % 4_900);
        state.terminal(operation);
        completed += 1;
    }

    state
        .durable_bytes
        .extend_from_slice(disk.read("telemetry-spool").expect("durable spool"));
    state
        .assert_clean(&[SECRET_CANARY], 5_000)
        .expect("release invariants");
    assert!(disk.total_bytes() <= 64 * 1024);
    assert!(clock.now_millis() > 1_000);
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}
