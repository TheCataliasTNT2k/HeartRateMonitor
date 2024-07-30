#![deny(
    unsafe_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::wildcard_enum_match_arm
)]
#![warn(
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable,
    clippy::pedantic,
    clippy::self_named_module_files,
    clippy::shadow_unrelated,
    clippy::str_to_string,
    clippy::dbg_macro,
    clippy::use_debug,
)]

use std::error::Error;
use std::process::exit;
use std::sync::{Arc, LazyLock};
use std::thread;
use std::time::Duration;

use chrono::Utc;
use log::{error, info, warn};
use poem::{EndpointExt, get, Route, Server};
use poem::listener::TcpListener;
use poem::middleware::Cors;
use tera::Tera;
use tokio::sync::RwLock;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{EnvFilter, fmt};

use crate::adaptors::{ChannelTransferObject, HrmState};
use crate::adaptors::hrm::HRM;
use crate::api::{heart_rate, index, list_templates, load_templates, reload_templates, template, ws};
use crate::config::MergedConfig;
use crate::csv_log::CSV_LOGGER;
use crate::shutdown_handler::{Shutdown, ShutdownHandler};
use crate::stdin::run as run_stdin;

mod config;
mod args;
mod api;
mod stdin;
mod csv_log;
mod shutdown_handler;
mod adaptors;

pub static CANCELLATION_TOKEN: LazyLock<CancellationToken> = LazyLock::new(CancellationToken::new);

/// Data which is available in all Poem routes
pub struct ProgramData {
    /// Merged config from [`args::Args`] and [`config::ProgramConfig`] (cli args take precedence if not [`None`])
    pub merged_config: Arc<RwLock<MergedConfig>>,
    /// All found [`tera::Tera`] templates + the default templates
    pub tera: RwLock<Tera>,
    /// The last HR data
    pub hr_data: Arc<RwLock<ChannelTransferObject>>,
}

#[allow(clippy::too_many_lines)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // configure a custom event formatter
    let format = fmt::format()
        .with_level(true)
        .without_time()
        .with_ansi(true); // use the `Compact` formatting style.


    fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .event_format(format)
        .init();

    let mut config = MergedConfig::load()?;

    let debug_active = config.args.debug_device;
    
    let data;
    if debug_active {
        // create a data object, which is available in all poem requests
        data = Arc::new(ProgramData {
            tera: RwLock::new(Tera::default()),
            merged_config: Arc::new(RwLock::new(config)),
            hr_data: Arc::new(RwLock::new(ChannelTransferObject {
                timestamp: Utc::now(),
                hr_state: None,
            })),
        });
        
    } else {
        // check some things, or exit
        if let Some(mut hrm_index) = config.args.hrm_index {
            if hrm_index == 0 {
                hrm_index = 1;
                config.args.hrm_index = Some(1);
            }
            #[allow(clippy::cast_possible_truncation)]
            if hrm_index > config.program_config.hrm_list.len() as u8 {
                error!("HRM Index is out of range (1 - {})!", config.program_config.hrm_list.len());
                exit(1);
            }
        }

        if config.enable_csv_log {
            if let Some(ref folder) = config.log_filepath {
                if config.enable_csv_log {
                    if !folder.exists() {
                        error!("Log folder \"{}\" does not exist!", folder.display());
                        exit(1);
                    }
                    if !folder.is_dir() {
                        error!("Log folder \"{}\" is not a folder!", folder.display());
                        exit(1);
                    }
                }
            }
        }

        let tera = if config.enable_http_server {
            if let Some(ref folder) = config.program_config.http_template_folder {
                if !folder.exists() {
                    error!("Template folder \"{}\" does not exist!", folder.display());
                    exit(1);
                }
                if !folder.is_dir() {
                    error!("Template folder \"{}\" is not a folder!", folder.display());
                    exit(1);
                }
            }
            load_templates(&config.program_config.http_template_folder, true).await?
        } else {
            Tera::default()
        };

        if !config.enable_http_server && !config.enable_csv_log {
            warn!("No http server and no csv logger active, exiting!");
            exit(0);
        }

        // create a data object, which is available in all poem requests
        data = Arc::new(ProgramData {
            tera: RwLock::new(tera),
            merged_config: Arc::new(RwLock::new(config)),
            hr_data: Arc::new(RwLock::new(ChannelTransferObject {
                timestamp: Utc::now(),
                hr_state: None,
            })),
        });
    }

    // watch stdin
    thread::spawn(run_stdin);

    // create a shutdown handler
    // it will run some cleanup hooks for structs when the program panics, receives a signal or exits normally
    let sh = ShutdownHandler::new();
    ShutdownHandler::create_watchers();
    let sh = Arc::new(sh);

    // create and start a HeartRate Manager, to observer heart rate
    HRM.register_shutdown_hook(Arc::clone(&sh)).await;
    tokio::spawn(HRM.run(Arc::clone(&data)));
    
    if debug_active {
        info!("Because \"debug device\" is active, server and logger are disabled.");
        CANCELLATION_TOKEN.cancelled().await;
        exit(0);
    }

    // create and start csv logger; store handle for joining later
    CSV_LOGGER.register_shutdown_hook(Arc::clone(&sh)).await;
    let csv_handle = tokio::spawn(CSV_LOGGER.run(Arc::clone(&data)));

    // start a loop to store new data in program data created above
    HrmState::storage_loop(Arc::clone(&data));

    // setup poem with all routes, middlewares etc
    let app = Route::new()
        .at("/", get(index))
        .at("/heart_rate", get(heart_rate))
        .at("/data", get(heart_rate))
        .at("/template", get(template))
        .at("/reload_templates", get(reload_templates))
        .at("/list_templates", get(list_templates))
        .at("/ws", get(ws))
        .at("/websocket", get(ws))
        .with(Cors::new())
        .data(Arc::clone(&data));

    if data.merged_config.read().await.enable_http_server {
        // if we want to have a http server
        // get host and port for http server
        let host = data.merged_config.read().await.program_config.http_host.clone().unwrap_or("127.0.0.1".to_owned());
        let port = data.merged_config.read().await.http_port;

        // run http server
        if let Err(error) = Server::new(TcpListener::bind((host, port)))
            .run_with_graceful_shutdown(
                app,
                CANCELLATION_TOKEN.cancelled(),
                Some(Duration::from_secs(1)),
            ).await {
            error!("{error}");
        }
    } else {
        // if we do not have a http server, join csv_handler
        let _ = csv_handle.await;
    };
    // drop shutdown handler to trigger all shutdown hooks for all structs
    drop(sh);
    sleep(Duration::from_secs(1)).await;
    info!("Exiting normally...");
    Ok(())
}