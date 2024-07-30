use std::fs::File;
use std::path::Path;
use std::process::exit;
use clap::{Parser};
use config::{Config, File as CFile};
use log::{error, info};
use mac_address::MacAddress;
use serde::{Deserialize, Serialize};
use serde_json::{to_writer_pretty};
use crate::args::Args;

/// Program config read from file
#[derive(Serialize, Deserialize, Debug, Default)]
#[allow(clippy::module_name_repetitions)]
#[serde(default)]
pub struct ProgramConfig {
    /// Stores all heart rate monitors, which were connected before
    #[serde(default)]
    pub hrm_list: Vec<Hrm>,

    /// If the http server should be enabled at all
    #[serde(default)]
    pub enable_http_server: Option<bool>,
    /// HTTP host to bind http server to
    #[serde(default)]
    pub http_host: Option<String>,
    /// HTTP port to bind to
    #[serde(default)]
    pub http_port: Option<u16>,
    
    /// Folder to search [`tera::Tera`] template files in
    #[serde(default)]
    pub http_template_folder: Option<Box<Path>>,
    
    // if csv logging should be enabled
    #[serde(default)]
    pub enable_csv_log: Option<bool>,
    /// Where to store the files
    #[serde(default)]
    pub csv_folder: Option<Box<Path>>,
}

impl ProgramConfig {
    /// Loads config from file
    pub fn load() -> anyhow::Result<Self> {
        Ok(
            Config::builder()
                .add_source(CFile::with_name("settings.json"))
                .build()?
                .try_deserialize()?
        )
    }

    /// Save the config to file.
    /// 
    /// needed to update `ProgramConfig::hrm_list`
    pub fn save(&self) -> anyhow::Result<()> {
        Ok(to_writer_pretty(File::create("settings.json")?, &self)?)
    }
    
    /// Stores a new device.
    /// 
    /// This will also save the file to disk.\
    /// Will do nothing if a device with same mac already exists.
    pub fn add_hrm(&mut self, hrm: Hrm) {
        if !self.hrm_list.iter().any(|d| d.mac == hrm.mac) {
            info!("Adding new device {}...", hrm.name);
            self.hrm_list.push(hrm);
            if let Err(error) = self.save() {
                error!("Error while saving config: {error}");
            };
        }
    }
}

/// Represents a specific previously connected heart rate monitor.
#[derive(Serialize, Deserialize, Debug)]
pub struct Hrm {
    /// Name of the monitor
    pub name: String,
    /// The bluetooth mac address to match
    pub mac: MacAddress,
    /// The internal id of the adapter to read values and parse them
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adaptor_id: Option<u16>
}

/// The merged configs from [`ProgramConfig`] and [`Args`]
/// 
/// Options set in args to anything else than [`None`] will override settings in `ProgramConfig` temporarily.
#[allow(clippy::module_name_repetitions)]
pub struct MergedConfig {
    /// The [`ProgramConfig`] object used for this config
    pub program_config: ProgramConfig,
    /// Start HTTP server
    pub enable_http_server: bool,
    /// HTTP port
    pub http_port: u16,
    /// Enable logging to csv file
    pub enable_csv_log: bool,
    /// Folder where the csv files will be stored
    pub log_filepath: Option<Box<Path>>,
    /// The cli [`Args`] object used for this config
    pub args: Args
}

impl MergedConfig {
    /// Loads the [`ProgramConfig`] and [`Args`] and merges them
    pub fn load() -> anyhow::Result<Self> {
        let cli = match Args::try_parse() {
            Ok(v) => {v}
            Err(err) => {
                println!("{err}");
                exit(0);
            }
        };
        let config = ProgramConfig::load()?;
        Ok(Self {
            program_config: ProgramConfig::load()?,
            args: cli.clone(),
            enable_http_server: cli.enable_http_server.or(config.enable_http_server).unwrap_or(false),
            http_port: cli.http_port.or(config.http_port).unwrap_or(8080),
            enable_csv_log: cli.enable_csv_log.or(config.enable_csv_log).unwrap_or(false),
            log_filepath: config.csv_folder,
        })
    }
}