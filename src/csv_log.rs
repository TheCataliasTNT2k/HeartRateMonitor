//! CSV Logger to write datapoints to file
//!
//! This csv logger listens for data on [`hrm::SENDER`] and caches all received values.
//! Every minute, all non saved data points are saved to a csv file.

use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::path::Path;
use std::sync::{Arc, LazyLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Local, Utc};
use log::{error, info, warn};
use tokio::sync::RwLock;
use tokio::time::sleep;

use crate::adaptors::{get_receiver, HrmState};
use crate::ProgramData;
use crate::shutdown_handler::{Shutdown, ShutdownHandler};

/// Static to allow access from "outside".
pub static CSV_LOGGER: LazyLock<CsvLogger> = LazyLock::new(|| CsvLogger {
    data: Arc::default(),
    filepath: RwLock::new(None),
    first_save: AtomicBool::from(true),
    started: AtomicBool::from(false),
    hook_registered: AtomicBool::from(false),
});

/// Logs the heart rate to csv.
pub struct CsvLogger {
    #[allow(clippy::type_complexity)]
    data: Arc<RwLock<VecDeque<(DateTime<Utc>, u16)>>>,
    first_save: AtomicBool,
    filepath: RwLock<Option<Box<Path>>>,
    started: AtomicBool,
    hook_registered: AtomicBool,
}

#[async_trait]
impl Shutdown for CsvLogger {
    /// Saves all data to the file on shutdown.
    async fn register_shutdown_hook(&self, shutdown_handler: Arc<ShutdownHandler>) {
        if self.hook_registered.swap(true, Ordering::Acquire) {
            warn!("Shutdown hook for csv logger already exists, aborting append.");
            return;
        }
        shutdown_handler.register_hook(
            Box::new(|| Box::pin(async {
                CSV_LOGGER.write_data().await;
            }))
        ).await;
    }
}

impl CsvLogger {
    /// Start running this logger.
    ///
    /// It will subscribe to [`hrm::SENDER`] to receive values and store them.
    /// Every minute, all not saved points will be appended to the csv file on disk.
    pub async fn run(&self, program_data: Arc<ProgramData>) {
        // if logging is disabled, return
        if !program_data.merged_config.read().await.enable_csv_log {
            return;
        }

        // else if logging was already started, show warning and return
        // this ensures, that the logger is not running multiple times
        if self.started.swap(true, Ordering::Acquire) {
            warn!("Csv logger started multiple times! Stopping all but one.");
            return;
        }

        // generate the filepath to log to
        match &program_data.merged_config.read().await.log_filepath {
            None => {
                warn!("Filepath for csv logger is not set, disabling it!");
                self.started.store(false, Ordering::Release);
                return;
            }
            Some(path) => {
                *self.filepath.write().await = Some(
                    Box::from(
                        path.join(
                            format!("heartrate-log-{}.csv", Utc::now().format("%Y-%m-%d %H:%M:%S"))
                        )
                    )
                );
            }
        }

        let data_clone = Arc::clone(&self.data);

        // spawn task to receive data and append it to unsaved data list
        tokio::spawn(async move {
            let mut receiver = get_receiver();
            loop {
                if let Ok(data) = receiver.recv().await {
                    if let Some(state) = data.hr_state {
                        match state {
                            HrmState::Disconnected => {}
                            HrmState::Ok(hr) => {
                                data_clone.write().await.push_back((data.timestamp, hr.hr));
                            }
                        }
                    }
                }
            }
        });

        // save unsaved data every minute
        loop {
            self.write_data().await;
            sleep(Duration::from_secs(60)).await;
        }
    }

    /// Writes all non saved points to the csv files and clears the buffer.
    async fn write_data(&self) {
        // if logger is not active, return
        if !self.started.load(Ordering::Acquire) {
            info!("Skipping saving of csv data, because logger not active.");
            return;
        }

        // get filepath or return
        info!("Saving csv data");
        let write_lock = self.filepath.read().await;
        let Some(filepath) = write_lock.as_ref() else {
            warn!("No filepath set for saving csv data");
            return;
        };

        // write lock must be held until "data.clear()", to prevent data loss
        // this also prevents a second thread from going beyond this point while one thread is saving data
        let mut data = self.data.write().await;

        // open file in append and create mode
        match OpenOptions::new().append(true).create(true).open(filepath) {
            Ok(file) => {
                let mut wtr = csv::Writer::from_writer(file);
                // if this is the first time we store data, add the column headers
                if self.first_save.load(Ordering::Acquire) {
                    // add header to record
                    if let Err(err) = wtr.write_record(["timestamp (utc)", "time (local)", "heart rate (bpm)"]) {
                        error!("Error while appending csv header: {err}");
                        return;
                    }
                    // flush changes to file
                    // do not remove here, because if we get errors later while appending actual data,
                    // the headers will be lost!
                    if let Err(err) = wtr.flush() {
                        error!("Could not write csv header to file: {err}");
                        return;
                    }
                    // prevent function from writing headers a second time
                    self.first_save.store(false, Ordering::Release);
                }

                // add all data to the csv writer
                for (time, hr) in data.iter() {
                    if let Err(err) = wtr.write_record(&[
                        time.timestamp().to_string(),
                        time.with_timezone(&Local::now().timezone()).format("%H:%M:%S").to_string(),
                        hr.to_string()
                    ]) {
                        error!("Error while appending csv data: {err}");
                    }
                }

                // flush writer to file
                if let Err(err) = wtr.flush() {
                    error!("Could not write csv data to file: {err}");
                    return;
                }

                // clear all collected data; we do not need it anymore, because we append to the file
                data.clear();
            }
            Err(err) => {
                error!("Error while saving csv file: {err}");
            }
        }
    }
}