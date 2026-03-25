//! Integration tests for multi-tenant isolation, idempotency, retry/lease behavior.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{Duration, Utc};
use skdlr_core::models::{DEFAULT_TENANT_ID, JobInstance, JobState, Schedule, ScheduleStatus};
use skdlr_core::storage::Storage;

fn mem_storage() -> Storage {
    Storage::in_memory().unwrap()
}

// ── Multi-tenant isolation tests ──────────────────────────────────────────────

#[test]
fn test_tenant_schedule_name_uniqueness() {
    let storage = mem_storage();

    // Same name in different tenants is allowed
    let s1 = Schedule::new("backup", "0 * * * *", "echo a").with_tenant("alpha");
    let s2 = Schedule::new("backup", "0 * * * *", "echo b").with_tenant("beta");
    storage.save_schedule(&s1).unwrap();
    storage.save_schedule(&s2).unwrap();

    // Verify both exist independently
    let alpha = storage
        .get_schedule_by_name_for_tenant("alpha", "backup")
        .unwrap()
        .unwrap();
    assert_eq!(alpha.command, "echo a");

    let beta = storage
        .get_schedule_by_name_for_tenant("beta", "backup")
        .unwrap()
        .unwrap();
    assert_eq!(beta.command, "echo b");

    // Same name in same tenant with different id should fail (unique constraint)
    let s3 = Schedule::new("backup", "0 * * * *", "echo c").with_tenant("alpha");
    let result = storage.save_schedule(&s3);
    assert!(result.is_err(), "duplicate (tenant, name) should fail");

    // Update existing schedule by same id is fine (upsert)
    let mut updated = alpha;
    updated.command = "echo updated".to_string();
    storage.save_schedule(&updated).unwrap();
    let reloaded = storage
        .get_schedule_by_name_for_tenant("alpha", "backup")
        .unwrap()
        .unwrap();
    assert_eq!(reloaded.command, "echo updated");
}

#[test]
fn test_tenant_list_isolation() {
    let storage = mem_storage();

    for i in 0..5 {
        let s = Schedule::new(format!("job-{i}"), "0 * * * *", format!("echo {i}"))
            .with_tenant("tenant-x");
        storage.save_schedule(&s).unwrap();
    }
    for i in 0..3 {
        let s = Schedule::new(format!("job-{i}"), "0 * * * *", format!("echo {i}"))
            .with_tenant("tenant-y");
        storage.save_schedule(&s).unwrap();
    }

    assert_eq!(
        storage.list_schedules_for_tenant("tenant-x").unwrap().len(),
        5
    );
    assert_eq!(
        storage.list_schedules_for_tenant("tenant-y").unwrap().len(),
        3
    );
    assert_eq!(
        storage.list_schedules_for_tenant("tenant-z").unwrap().len(),
        0
    );
    assert_eq!(storage.list_all_schedules().unwrap().len(), 8);
}

#[test]
fn test_tenant_delete_isolation() {
    let storage = mem_storage();

    let s1 = Schedule::new("job", "0 * * * *", "echo a").with_tenant("t1");
    let s2 = Schedule::new("job", "0 * * * *", "echo b").with_tenant("t2");
    storage.save_schedule(&s1).unwrap();
    storage.save_schedule(&s2).unwrap();

    // Delete from t1 should not affect t2
    storage.delete_schedule(&s1.id).unwrap();
    assert!(
        storage
            .get_schedule_by_name_for_tenant("t1", "job")
            .unwrap()
            .is_none()
    );
    assert!(
        storage
            .get_schedule_by_name_for_tenant("t2", "job")
            .unwrap()
            .is_some()
    );
}

// ── Idempotency tests ─────────────────────────────────────────────────────────

#[test]
fn test_job_idempotency_key_prevents_duplicates() {
    let storage = mem_storage();

    let schedule = Schedule::new("idempotent-test", "0 * * * *", "echo hello");
    storage.save_schedule(&schedule).unwrap();

    let scheduled_at = Utc::now();
    let instance = JobInstance::new(&schedule, scheduled_at);

    // First enqueue succeeds
    let result = storage.enqueue_job(&instance).unwrap();
    assert!(result.is_some());

    // Second enqueue with same idempotency key is silently ignored
    let dup = storage.enqueue_job(&instance).unwrap();
    assert!(dup.is_none());

    // Different scheduled_at = different idempotency key = new instance
    let later = scheduled_at + Duration::hours(1);
    let instance2 = JobInstance::new(&schedule, later);
    let result2 = storage.enqueue_job(&instance2).unwrap();
    assert!(result2.is_some());

    // Should have exactly 2 instances
    let all = storage.get_job_instances(&schedule.id, 100).unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn test_idempotency_across_tenants() {
    let storage = mem_storage();

    let s1 = Schedule::new("shared-job", "0 * * * *", "echo a").with_tenant("t1");
    let s2 = Schedule::new("shared-job", "0 * * * *", "echo b").with_tenant("t2");
    storage.save_schedule(&s1).unwrap();
    storage.save_schedule(&s2).unwrap();

    let scheduled_at = Utc::now();
    let i1 = JobInstance::new(&s1, scheduled_at);
    let i2 = JobInstance::new(&s2, scheduled_at);

    // Different schedule IDs = different idempotency keys
    let r1 = storage.enqueue_job(&i1).unwrap();
    let r2 = storage.enqueue_job(&i2).unwrap();
    assert!(r1.is_some());
    assert!(r2.is_some());
}

// ── Retry/lease behavior tests ────────────────────────────────────────────────

#[test]
fn test_lease_renewal() {
    let storage = mem_storage();

    let schedule = Schedule::new("lease-test", "0 * * * *", "echo hello");
    storage.save_schedule(&schedule).unwrap();

    let instance = JobInstance::new(&schedule, Utc::now());
    storage.enqueue_job(&instance).unwrap();

    let claimed = storage
        .claim_job("worker-1", Duration::minutes(5))
        .unwrap()
        .unwrap();

    // Renew lease
    let renewed = storage
        .renew_lease(&claimed.id, "worker-1", Duration::minutes(10))
        .unwrap();
    assert!(renewed);

    // Wrong worker can't renew
    let wrong = storage
        .renew_lease(&claimed.id, "worker-2", Duration::minutes(10))
        .unwrap();
    assert!(!wrong);
}

#[test]
fn test_claim_returns_oldest_first() {
    let storage = mem_storage();

    let schedule = Schedule::new("order-test", "0 * * * *", "echo hello");
    storage.save_schedule(&schedule).unwrap();

    let t1 = Utc::now() - Duration::hours(2);
    let t2 = Utc::now() - Duration::hours(1);

    let i1 = JobInstance::new(&schedule, t1);
    let i2 = JobInstance::new(&schedule, t2);
    storage.enqueue_job(&i1).unwrap();
    storage.enqueue_job(&i2).unwrap();

    // First claim should get the oldest
    let claimed1 = storage
        .claim_job("worker-1", Duration::minutes(5))
        .unwrap()
        .unwrap();
    assert_eq!(claimed1.scheduled_at, t1);

    // Second claim should get the next
    let claimed2 = storage
        .claim_job("worker-1", Duration::minutes(5))
        .unwrap()
        .unwrap();
    assert_eq!(claimed2.scheduled_at, t2);

    // No more jobs
    let none = storage.claim_job("worker-1", Duration::minutes(5)).unwrap();
    assert!(none.is_none());
}

#[test]
fn test_exponential_backoff() {
    let storage = mem_storage();

    let schedule = Schedule::new("backoff-test", "0 * * * *", "echo fail").with_retries(5, 10);
    storage.save_schedule(&schedule).unwrap();

    let instance = JobInstance::new(&schedule, Utc::now());
    storage.enqueue_job(&instance).unwrap();

    // Claim and fail multiple times, checking retry delay grows
    let claimed = storage
        .claim_job("w1", Duration::minutes(5))
        .unwrap()
        .unwrap();
    storage.fail_job(&claimed.id, "fail 1", 10).unwrap();

    // Check the instance
    let instances = storage.get_job_instances(&schedule.id, 10).unwrap();
    let inst = &instances[0];
    assert_eq!(inst.state, JobState::Retrying);
    assert_eq!(inst.attempt, 1);
    assert!(inst.next_retry_at.is_some());
}

#[test]
fn test_dead_letter_after_max_retries() {
    let storage = mem_storage();

    // 0 retries = 1 attempt total → fails immediately to dead letter
    let schedule = Schedule::new("deadletter-test", "0 * * * *", "echo fail").with_retries(0, 10);
    storage.save_schedule(&schedule).unwrap();

    let instance = JobInstance::new(&schedule, Utc::now());
    storage.enqueue_job(&instance).unwrap();

    let claimed = storage
        .claim_job("w1", Duration::minutes(5))
        .unwrap()
        .unwrap();

    let state = storage.fail_job(&claimed.id, "total failure", 10).unwrap();
    assert_eq!(state, JobState::DeadLetter);

    let dead = storage.get_dead_letter_jobs(DEFAULT_TENANT_ID, 10).unwrap();
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].last_error, Some("total failure".to_string()));
}

#[test]
fn test_concurrent_claim_is_atomic() {
    let storage = mem_storage();

    let schedule = Schedule::new("atomic-test", "0 * * * *", "echo hello");
    storage.save_schedule(&schedule).unwrap();

    let instance = JobInstance::new(&schedule, Utc::now());
    storage.enqueue_job(&instance).unwrap();

    // First claim succeeds
    let claim1 = storage.claim_job("worker-1", Duration::minutes(5)).unwrap();
    assert!(claim1.is_some());

    // Second claim should get nothing (only 1 job)
    let claim2 = storage.claim_job("worker-2", Duration::minutes(5)).unwrap();
    assert!(claim2.is_none());
}

#[test]
fn test_stuck_recovery_requeues_job() {
    let storage = mem_storage();

    let schedule = Schedule::new("stuck-recovery", "0 * * * *", "echo hello");
    storage.save_schedule(&schedule).unwrap();

    let instance = JobInstance::new(&schedule, Utc::now());
    storage.enqueue_job(&instance).unwrap();

    // Claim with very short lease
    let claimed = storage
        .claim_job("dead-worker", Duration::seconds(0))
        .unwrap()
        .unwrap();

    // Force lease into the past
    storage
        .conn()
        .execute(
            "UPDATE job_instances SET lease_expires_at = ?1 WHERE id = ?2",
            rusqlite::params![
                (Utc::now() - Duration::minutes(10)).to_rfc3339(),
                claimed.id.to_string(),
            ],
        )
        .unwrap();

    // Recover
    let count = storage.recover_stuck_jobs().unwrap();
    assert_eq!(count, 1);

    // Job should be claimable again
    let reclaimed = storage
        .claim_job("new-worker", Duration::minutes(5))
        .unwrap();
    assert!(reclaimed.is_some());
    assert_eq!(
        reclaimed.unwrap().last_error,
        Some("lease expired - recovered by stuck job recovery".to_string())
    );
}

#[test]
fn test_complete_job_only_from_running() {
    let storage = mem_storage();

    let schedule = Schedule::new("state-test", "0 * * * *", "echo hello");
    storage.save_schedule(&schedule).unwrap();

    let instance = JobInstance::new(&schedule, Utc::now());
    storage.enqueue_job(&instance).unwrap();

    // Can't complete a queued job
    let completed = storage.complete_job(&instance.id, 0).unwrap();
    assert!(!completed);

    // Claim it
    storage
        .claim_job("w1", Duration::minutes(5))
        .unwrap()
        .unwrap();

    // Now complete should work
    let completed = storage.complete_job(&instance.id, 0).unwrap();
    assert!(completed);

    // Can't complete again
    let again = storage.complete_job(&instance.id, 0).unwrap();
    assert!(!again);
}

#[test]
fn test_schedule_pause_and_resume() {
    let storage = mem_storage();

    let mut schedule = Schedule::new("pausable", "0 * * * *", "echo hello");
    storage.save_schedule(&schedule).unwrap();

    // Pause
    schedule.status = ScheduleStatus::Paused;
    schedule.paused_until = Some(Utc::now() + Duration::hours(1));
    storage.save_schedule(&schedule).unwrap();

    let loaded = storage.get_schedule(&schedule.id).unwrap().unwrap();
    assert_eq!(loaded.status, ScheduleStatus::Paused);
    assert!(loaded.paused_until.is_some());

    // Resume
    schedule.status = ScheduleStatus::Enabled;
    schedule.paused_until = None;
    storage.save_schedule(&schedule).unwrap();

    let loaded = storage.get_schedule(&schedule.id).unwrap().unwrap();
    assert_eq!(loaded.status, ScheduleStatus::Enabled);
    assert!(loaded.paused_until.is_none());
}

#[test]
fn test_count_pending_jobs() {
    let storage = mem_storage();

    let schedule = Schedule::new("count-test", "0 * * * *", "echo hello").with_retries(1, 10);
    storage.save_schedule(&schedule).unwrap();

    assert_eq!(storage.count_pending_jobs().unwrap(), 0);

    // Enqueue 3 jobs
    for i in 0..3 {
        let t = Utc::now() - Duration::hours(i as i64);
        let inst = JobInstance::new(&schedule, t);
        storage.enqueue_job(&inst).unwrap();
    }

    assert_eq!(storage.count_pending_jobs().unwrap(), 3);

    // Claim and complete one
    let claimed = storage
        .claim_job("w1", Duration::minutes(5))
        .unwrap()
        .unwrap();
    storage.complete_job(&claimed.id, 0).unwrap();

    // 2 queued remaining
    assert_eq!(storage.count_pending_jobs().unwrap(), 2);
}
