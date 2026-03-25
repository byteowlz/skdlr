//! Central scheduler loop for the service daemon.
//!
//! The scheduler continuously:
//! 1. Checks enabled schedules for due jobs
//! 2. Enqueues job instances (idempotent)
//! 3. Claims and dispatches queued jobs
//! 4. Handles completion, retries, and dead-lettering
//! 5. Recovers stuck jobs with expired leases

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use tokio::sync::watch;
use uuid::Uuid;

use crate::dispatcher::{DispatchResult, Dispatcher};
use crate::error::Result;
use crate::models::{JobInstance, JobState, Schedule, ScheduleKind, ScheduleStatus};
use crate::storage::Storage;

/// Configuration for the scheduler loop.
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// How often to check for due schedules (seconds).
    pub poll_interval_secs: u64,

    /// Lease duration for claimed jobs (seconds).
    pub lease_duration_secs: u64,

    /// How often to check for stuck jobs (seconds).
    pub stuck_check_interval_secs: u64,

    /// Worker ID for this scheduler instance.
    pub worker_id: String,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 10,
            lease_duration_secs: 300,
            stuck_check_interval_secs: 60,
            worker_id: format!("worker-{}", Uuid::new_v4()),
        }
    }
}

/// The central scheduler that orchestrates job execution.
pub struct Scheduler {
    storage: Arc<Mutex<Storage>>,
    dispatcher: Arc<dyn Dispatcher>,
    config: SchedulerConfig,
}

impl std::fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scheduler")
            .field("config", &self.config)
            .field("dispatcher", &self.dispatcher.name())
            .finish()
    }
}

impl Scheduler {
    /// Creates a new scheduler.
    pub fn new(
        storage: Arc<Mutex<Storage>>,
        dispatcher: Arc<dyn Dispatcher>,
        config: SchedulerConfig,
    ) -> Self {
        Self {
            storage,
            dispatcher,
            config,
        }
    }

    /// Runs the scheduler loop until shutdown is signaled.
    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) -> Result<()> {
        tracing::info!(
            worker_id = %self.config.worker_id,
            dispatcher = self.dispatcher.name(),
            poll_interval = self.config.poll_interval_secs,
            "Scheduler starting"
        );

        let poll_interval = tokio::time::Duration::from_secs(self.config.poll_interval_secs);
        let stuck_interval =
            tokio::time::Duration::from_secs(self.config.stuck_check_interval_secs);

        let mut poll_ticker = tokio::time::interval(poll_interval);
        let mut stuck_ticker = tokio::time::interval(stuck_interval);

        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        tracing::info!("Scheduler shutting down");
                        break;
                    }
                }
                _ = poll_ticker.tick() => {
                    if let Err(e) = self.tick().await {
                        tracing::error!(error = %e, "Scheduler tick failed");
                    }
                }
                _ = stuck_ticker.tick() => {
                    if let Err(e) = self.recover_stuck() {
                        tracing::error!(error = %e, "Stuck job recovery failed");
                    }
                }
            }
        }

        Ok(())
    }

    /// Single scheduler tick: enqueue due jobs and process queue.
    async fn tick(&self) -> Result<()> {
        // 1. Enqueue due jobs
        self.enqueue_due_jobs()?;

        // 2. Process queued jobs
        self.process_queue().await?;

        Ok(())
    }

    /// Acquires the storage lock.
    fn db(&self) -> std::sync::MutexGuard<'_, Storage> {
        self.storage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Checks all enabled schedules and enqueues job instances for due ones.
    fn enqueue_due_jobs(&self) -> Result<()> {
        let schedules = self.db().list_all_schedules()?;
        let now = Utc::now();

        for schedule in &schedules {
            if schedule.status != ScheduleStatus::Enabled {
                continue;
            }

            // Check if paused
            if schedule
                .paused_until
                .is_some_and(|paused_until| now < paused_until)
            {
                continue;
            }

            if let Some(scheduled_at) = Self::next_run_time(&schedule.kind, now) {
                // Only enqueue if the scheduled time is now or in the past
                // (within the poll interval window)
                if scheduled_at <= now {
                    let instance = JobInstance::new(schedule, scheduled_at);
                    match self.db().enqueue_job(&instance) {
                        Ok(Some(_)) => {
                            tracing::debug!(
                                schedule = %schedule.name,
                                tenant = %schedule.tenant_id,
                                scheduled_at = %scheduled_at,
                                "Enqueued job instance"
                            );
                        }
                        Ok(None) => {
                            // Duplicate — already enqueued
                        }
                        Err(e) => {
                            tracing::warn!(
                                schedule = %schedule.name,
                                error = %e,
                                "Failed to enqueue job"
                            );
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Processes the job queue: claims and dispatches jobs.
    async fn process_queue(&self) -> Result<()> {
        let lease_duration = chrono::Duration::seconds(self.config.lease_duration_secs as i64);

        // Process jobs one at a time (could be parallelized later)
        loop {
            let claimed = self
                .db()
                .claim_job(&self.config.worker_id, lease_duration)?;

            let Some(instance) = claimed else {
                break; // No more jobs to process
            };

            tracing::info!(
                job_id = %instance.id,
                schedule_id = %instance.schedule_id,
                attempt = instance.attempt,
                "Processing job"
            );

            // Look up the schedule
            let Some(schedule) = self.db().get_schedule(&instance.schedule_id)? else {
                tracing::warn!(
                    job_id = %instance.id,
                    schedule_id = %instance.schedule_id,
                    "Schedule not found for job instance"
                );
                let _ = self.db().complete_job(&instance.id, -1);
                continue;
            };

            // Dispatch the job
            let result = self.dispatcher.dispatch(&schedule, &instance).await;

            match result {
                Ok(dispatch_result) => {
                    self.handle_result(&instance, &schedule, &dispatch_result)?;
                }
                Err(e) => {
                    tracing::error!(
                        job_id = %instance.id,
                        error = %e,
                        "Dispatch error"
                    );
                    let _ = self.db().fail_job(
                        &instance.id,
                        &format!("dispatch error: {e}"),
                        schedule.retry_delay_secs,
                    );
                }
            }
        }

        Ok(())
    }

    /// Handles the result of a dispatched job.
    fn handle_result(
        &self,
        instance: &JobInstance,
        schedule: &Schedule,
        result: &DispatchResult,
    ) -> Result<()> {
        if result.is_success() {
            tracing::info!(
                job_id = %instance.id,
                schedule = %schedule.name,
                "Job succeeded"
            );
            self.db().complete_job(&instance.id, result.exit_code)?;
        } else {
            let error_msg = result
                .error_message()
                .unwrap_or_else(|| format!("exit code {}", result.exit_code));

            tracing::warn!(
                job_id = %instance.id,
                schedule = %schedule.name,
                exit_code = result.exit_code,
                error = %error_msg,
                "Job failed"
            );

            let new_state =
                self.db()
                    .fail_job(&instance.id, &error_msg, schedule.retry_delay_secs)?;

            match new_state {
                JobState::Retrying => {
                    tracing::info!(
                        job_id = %instance.id,
                        attempt = instance.attempt + 1,
                        max = instance.max_attempts,
                        "Job scheduled for retry"
                    );
                }
                JobState::DeadLetter => {
                    tracing::error!(
                        job_id = %instance.id,
                        schedule = %schedule.name,
                        attempts = instance.max_attempts,
                        "Job moved to dead letter queue"
                    );
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Recovers jobs with expired leases.
    fn recover_stuck(&self) -> Result<()> {
        let recovered = self.db().recover_stuck_jobs()?;
        if recovered > 0 {
            tracing::warn!(count = recovered, "Recovered stuck jobs");
        }
        Ok(())
    }

    /// Calculates the most recent scheduled time that should have triggered.
    fn next_run_time(kind: &ScheduleKind, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match kind {
            ScheduleKind::Recurring { cron_expr } => Self::most_recent_cron_time(cron_expr, now),
            ScheduleKind::OneOff { run_at } => {
                if *run_at <= now {
                    Some(*run_at)
                } else {
                    None
                }
            }
        }
    }

    /// Gets the most recent time a cron expression should have fired.
    fn most_recent_cron_time(cron_expr: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        use cron::Schedule as CronSchedule;
        use std::str::FromStr;

        let expr = if cron_expr.split_whitespace().count() == 5 {
            format!("0 {cron_expr}")
        } else {
            cron_expr.to_string()
        };

        let sched = CronSchedule::from_str(&expr).ok()?;

        // Find the most recent past occurrence
        // We look back 24h max to avoid infinite search
        let lookback = now - chrono::Duration::hours(24);
        sched.after(&lookback).take_while(|t| *t <= now).last()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_most_recent_cron_time() {
        // "every hour at :00" — should find the most recent one
        let now = Utc::now();
        let result = Scheduler::most_recent_cron_time("0 * * * *", now);
        assert!(result.is_some());
        let recent = result.unwrap();
        // Should be within the last hour
        assert!(now - recent < chrono::Duration::hours(1));
    }

    #[test]
    fn test_next_run_time_one_off() {
        let now = Utc::now();

        // Past one-off should trigger
        let past = now - chrono::Duration::hours(1);
        let kind = ScheduleKind::one_off(past);
        assert!(Scheduler::next_run_time(&kind, now).is_some());

        // Future one-off should not trigger
        let future = now + chrono::Duration::hours(1);
        let kind = ScheduleKind::one_off(future);
        assert!(Scheduler::next_run_time(&kind, now).is_none());
    }
}
