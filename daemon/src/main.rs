use rensen_lib::config::*;
use rensen_lib::traits::*;
use rensen_lib::logging::*;

pub mod scheduler;
pub mod utils;
pub mod tasks;

use crate::scheduler::*;

use cron::Schedule;
use std::sync::Arc;
use std::path::PathBuf;
use std::str::FromStr;
use tokio::sync::Mutex;

/// Gets all cron schedules from host configs and places them into a vector with associated
/// hostname (WSchedule)
fn parse_schedules(global_config: &GlobalConfig, settings: &Settings) -> Result<Vec<Arc<WSchedule>>, Trap> {
    let mut schedules: Vec<Arc<WSchedule>> = Vec::new();
    for host in settings.hosts.iter() {
        if host.hostname == "dummy" { continue }; // Skip dummy host
        if let Some(cron_schedule) = &host.config.cron_schedule {
            println!("Cron: {}", cron_schedule);

            // Parse cron expression and push to vector which will await its time for exec
            match Schedule::from_str(cron_schedule) {
                Ok(schedule) => {
                    let host_schedule = Arc::new(WSchedule { host: host.clone().into(), schedule });
                    println!("host_schedule: {:?}", host_schedule);
                    schedules.push(host_schedule);
                },
                Err(err) => {
                    log_trap(&global_config, &Trap::InvalidInput(format!("Invalid Cron Expression for `{}`: {}", host.hostname, err)));
                }
            }
        } else {
            // Defaults cron to midnight every day if parsing fails
            log_trap(&global_config, &Trap::Missing(format!("Missing cron_schedule for `{}`: Defaulting to `0 0 * * *`", &host.hostname)));
            let host_schedule = Arc::new(WSchedule {
                host: host.clone().into(),
                schedule: Schedule::from_str("0 0 0 * *").unwrap(),
            });

            schedules.push(host_schedule);
            continue;
        }
    }

    Ok(schedules)
}

#[tokio::main]
async fn main() -> Result<(), Trap> {
    let global_config_path = PathBuf::from("/etc/rensen/rensen_config.yml");
    let global_config: GlobalConfig = GlobalConfig::deserialize_yaml(&global_config_path)
        .map_err(|err| Trap::FS(format!("Could not deserialize Global Config: {}", err)))?;

    let settings = Settings::deserialize_yaml(&global_config.hosts)
        .map_err(|err| Trap::FS(format!("Could not deserialize Settings @ {:?}: {}", global_config.hosts, err)))?;

    let schedules = parse_schedules(&global_config, &settings)?;
    let backup_scheduler = Arc::new(Mutex::new(Scheduler::from(Arc::new(global_config.clone()), settings, schedules)));

    /* --------- */
    /* Scheduler */
    /* --------- */

    let scheduler_global_config = global_config.clone();
    // Clone Arc for run_scheduler
    let backup_scheduler_clone = Arc::clone(&backup_scheduler);

    // Spawn run_scheduler on a separate thread
    let scheduler_task = tokio::spawn(async move {
        // Clone the Arc and lock the Mutex
        let mut scheduler_guard = backup_scheduler_clone.lock().await;
        if let Err(err) = scheduler_guard.run_scheduler().await {
            log_trap(&scheduler_global_config, &Trap::Scheduler(format!("Could not start scheduler: {:?}", err)));
            std::mem::drop(scheduler_global_config);
        }
    });

    /* ---------*/
    /* Executor */
    /* ---------*/

    let executor_global_config = global_config.clone();
    // Clone Arc for run_task
    let executor_backup_scheduler_clone = Arc::clone(&backup_scheduler);
    // Spawn run_task on new thread
    let task_executor = tokio::spawn(async move {
        // Clone the Arc and lock the Mutex
        let mut executor_scheduler_guard = executor_backup_scheduler_clone.lock().await;
        if let Err(err) = executor_scheduler_guard.run_executor().await {
            log_trap(&executor_global_config, &Trap::Scheduler(format!("Could not start scheduler's executor: {:?}", err)));
            std::mem::drop(executor_global_config);
        }
    });

    // Finishing tasks
    if let Err(err) = tokio::try_join!(scheduler_task, task_executor) {
        eprintln!("Error occurred while running tasks: {:?}", err);
    }

    Ok(())
}

#[cfg(test)]
#[test]
fn test_cron() {
    let cron_str = "* 0 0 * * *";
    let schedule = Schedule::from_str(cron_str).unwrap();
}

