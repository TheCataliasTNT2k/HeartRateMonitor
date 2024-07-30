//! Command line args parser

use clap::Parser;
use mac_address::MacAddress;

/// Capture program arguments as settings.
/// 
/// All arguments, which are not [`None`] will override settings set in the [`config::ProgramConfig`](crate::config::ProgramConfig).
#[derive(Parser, Debug, Clone)]
#[command(version, about, long_about = None)]
#[allow(clippy::struct_excessive_bools)]
pub struct Args {
    /// Enable HTTP server
    #[clap(long)]
    pub enable_http_server: Option<bool>,
    /// HTTP port
    #[clap(long)]
    pub http_port: Option<u16>,

    /// Enable csv logging
    #[clap(long)]
    pub enable_csv_log: Option<bool>,

    /// Pair new device and use it noninteractively, instead of connecting to an already known one
    #[clap(default_value = "false", long, action = clap::ArgAction::SetTrue)]
    pub accept_new_device: bool,

    /// HRM mac to use / pair (overrides "hrm_index")
    #[clap(long)]
    pub hrm_mac: Option<MacAddress>,
    /// HRM index to use (first is 1); will be ignored, if "accept_new_device" is active
    #[clap(long)]
    pub hrm_index: Option<u8>,

    /// Pin chosen device for reconnections (set this flag, if multiple devices are used simultaneously)
    #[clap(default_value = "false", long, action = clap::ArgAction::SetTrue)]
    pub pin_device: bool,
    /// Rescan non interactively
    #[clap(default_value = "false", long, action = clap::ArgAction::SetTrue)]
    pub noninteractive_rescan: bool,
    
    /// Debug device; dumps EVERYTHING for the connected device in STDOUT
    #[clap(default_value = "false", long, action = clap::ArgAction::SetTrue)]
    pub debug_device: bool
}