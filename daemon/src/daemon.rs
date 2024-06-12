use rensen_lib::backup::rsync::Sftp;
use rensen_lib::config::*;
use rensen_lib::traits::*;
use rensen_lib::logging::*;
use rensen_lib::record::*;

use chrono::{Local, Timelike, SecondsFormat};
use cron::Schedule;
use tokio::time::{interval, Duration};
use std::sync::Arc;
use std::collections::VecDeque;

#[derive(Debug)]
struct TaskQueue<T> {
    tasks: VecDeque<T>,
}

impl<T> TaskQueue<T> {
    fn new() -> Self {
        TaskQueue {
            tasks: VecDeque::new()
        }
    }

    fn pushb(&mut self, val: T) {
        self.tasks.push_back(val);
    }

    fn popf(&mut self) -> Option<T> {
        self.tasks.pop_front()
    }

    fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    fn len(&self) -> usize {
        self.tasks.len()
    }

    fn peek(&self) -> Option<&T> {
        self.tasks.front()
    }
}

// Struct for holding the host data with it's associate schedul
#[derive(Debug)]
pub struct HostSchedule {
    pub host: Arc<Host>, 
    pub schedule: Schedule,
}

// Struct for running the actual backup task
#[derive(Debug)]
pub struct BackupTask {
    pub global_config: Arc<GlobalConfig>, 
    pub host: Arc<Host>, 
}

impl BackupTask {

    /// Performs backup task using the rensen sftp-backup lib
    async fn run(&self) -> Result<(), Trap> {

        let hostname = &self.host.hostname;
        let inc = true;
        let host_config = &self.host.config;

        let record_path = host_config.destination
            .join(&host_config.identifier)
            .join(".records")
            .join("record.json");

        let record = Record::deserialize_json(&record_path)
            .map_err(|err| Trap::FS(format!("Could not read record for host `{}`: {}", hostname, err)))?;

        let mut sftp = Sftp::new(&host_config, &self.global_config, record, inc);

        sftp.incremental = inc;
        sftp.backup()?;

        Ok(())
    }
}

pub struct BackupScheduler {
    pub global_config: Arc<GlobalConfig>, 
    pub settings: Settings,
    pub schedules: Vec<Arc<HostSchedule>>,
    queue: TaskQueue<BackupTask>
}

impl BackupScheduler {
    pub fn from(global_config: Arc<GlobalConfig>, settings: Settings, schedules: Vec<Arc<HostSchedule>>) -> Self {
        BackupScheduler { global_config, settings, schedules, queue: TaskQueue::new() }
    }

    /// Checking according to the hosts's schedule if it is time to
    /// backup at this moment.
    fn should_run(&self, now: &chrono::DateTime<Local>, host_schedule: &HostSchedule) -> bool {
        let current_time = now
        .with_second(0).unwrap()
        .with_nanosecond(0).unwrap();

        let mut upcoming_times = host_schedule.schedule.upcoming(Local).take(1);

        if let Some(scheduled_time) = upcoming_times.next() {
            println!(
                "Current time: {} (h: {}, m: {}, s: {}), Scheduled time: {} (h: {}, m: {}, s: {})",
                current_time.to_rfc3339_opts(SecondsFormat::Secs, true),
                current_time.hour(), current_time.minute(), current_time.second(),
                scheduled_time.to_rfc3339_opts(SecondsFormat::Secs, true),
                scheduled_time.hour(), scheduled_time.minute(), scheduled_time.second()
            );

            // Compare up to minutes precision
            return current_time == scheduled_time.with_second(0).unwrap().with_nanosecond(0).unwrap();
        }
        false
    }

    /// Looping through the schedules and running eventual backup tasks
    /// when self.should_run() == true
    /// Will wait 60 seconds between each check
    pub async fn run_scheduler(&self) -> Result<(), Trap> {
        let mut interval = interval(Duration::from_secs(60));

        println!("{:?}", self.schedules);

        loop {

            // Checking every interval if it's time
            interval.tick().await;
            let now = Local::now();

            for host_schedule in self.schedules.iter() {
                if self.should_run(&now, &host_schedule) {
                    println!("Should run now");
                    let global_config_clone = Arc::clone(&self.global_config);
                    let host = Arc::clone(&host_schedule.host); 
                    let backup_task = BackupTask { global_config: global_config_clone, host };

                    // Spawning new thread as it's time for backupping
                    tokio::spawn(async move {
                        if let Err(err) = backup_task.run().await {
                            log_trap(&backup_task.global_config, &err); 
                        }
                    });
                }
            }
        }
    }
}

