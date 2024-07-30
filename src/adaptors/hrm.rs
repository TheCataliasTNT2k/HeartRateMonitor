use std::collections::HashSet;
use std::io;
use std::io::Write;
use std::sync::{Arc, LazyLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::anyhow;
use async_trait::async_trait;
use btleplug::api::{BDAddr, Central, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::Manager;
use itertools::Itertools;
use log::{debug, error, info, warn};
use mac_address::MacAddress;
use tokio::sync::RwLock;
use tokio::time;
use tokio::time::sleep;

use crate::adaptors::{Adaptor, find_matching_adaptor, FoundDevice};
use crate::adaptors::adaptor_debug::AdaptorDebug;
use crate::ProgramData;
use crate::shutdown_handler::{Shutdown, ShutdownHandler};
use crate::stdin::next_line;

// storage for HRM to be accessible from "outside"
pub static HRM: LazyLock<HrManager> = LazyLock::new(|| HrManager {
    connected_device: Arc::default(),
    hook_registered: AtomicBool::new(false),
});


pub struct HrManager {
    connected_device: Arc<RwLock<Option<Arc<dyn Adaptor>>>>,
    hook_registered: AtomicBool,
}

#[async_trait]
impl Shutdown for HrManager {
    async fn register_shutdown_hook(&self, shutdown_handler: Arc<ShutdownHandler>) {
        if self.hook_registered.swap(true, Ordering::Acquire) {
            warn!("Shutdown hook for heart rate manager already exists, aborting append.");
            return;
        }

        shutdown_handler.register_hook(
            Box::new(|| Box::pin(async {
                if let Some(device) = HRM.connected_device.read().await.as_ref() {
                    info!("Disconnecting from device...");
                    let () = device.shutdown().await;
                }
            }))
        ).await;
    }
}

impl HrManager {
    pub async fn run(&self, program_data: Arc<ProgramData>) {
        loop {
            // search for existing devices
            let devices = match self.search(&program_data).await {
                Ok(v) => { v }
                Err(err) => {
                    error!("Got error while searching for devices; retrying in 1 second: {}", err);
                    sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };
            if devices.is_empty() {
                warn!("Found no devices at all! Repeating search in 1 second...");
                sleep(Duration::from_secs(1)).await;
                continue;
            }

            // let the user choose one (or chose automatically, if configured)
            let was_connected = self.connected_device.read().await.is_some();
            let Some(device) = self.choose_device(devices, was_connected, &program_data).await else { continue };

            // try to connect
            if !device.peripheral.is_connected().await.unwrap_or(false) {
                info!("Trying to connect to {:?}...", device.name);
                if let Err(err) = device.peripheral.connect().await {
                    error!("Could not connect to {} because of {:?}!", device.name, err);
                    continue;
                }
            }
            if !device.peripheral.is_connected().await.unwrap_or(false) {
                error!("Connection to {} failed; check, that your device is not connected to another host!", device.name);
                continue;
            }

            if program_data.merged_config.read().await.args.debug_device {
                match AdaptorDebug::try_wrap(Arc::new(device)).await {
                    Ok(Some(dev)) => {
                        if let Err(err) = dev.heartbeat_loop().await {
                            error!("Error while running heart rate loop for debug device: {err}");
                        };
                        continue;
                    }
                    Err(err) => {
                        error!("Error while creating debug device: {err}");
                        continue;
                    }
                    _ => {
                        continue;
                    }
                }
            }

            // check if device is compatible
            let addr = MacAddress::from(device.addr.into_inner());
            let adaptor = match find_matching_adaptor(
                &device,
                program_data
                    .merged_config
                    .read()
                    .await
                    .program_config
                    .hrm_list
                    .iter()
                    .find(|d| d.mac == addr)
            ).await {
                Ok(Some(device)) => {
                    device
                }
                Ok(None) => {
                    warn!("No matching adaptor could be found for device {}. You may need to add it to the config manually.", device.name);
                    continue;
                }
                Err(error) => {
                    error!("Error while matching adaptor: {error}");
                    continue;
                }
            };
            info!("Found matching peripheral {:?}...", device.name);

            // store new device in config file
            if !device.is_known {
                program_data.merged_config.write().await.program_config.add_hrm(adaptor.to_hrm().await);
            }
            let clone = Arc::clone(&adaptor);
            *self.connected_device.write().await = Some(adaptor);

            if let Err(error) = clone.heartbeat_loop().await {
                error!("Error in heartbeat loop: {error}");
            }
        }
    }

    async fn search(&self, program_data: &Arc<ProgramData>) -> anyhow::Result<Vec<FoundDevice>> {
        let mut filter: HashSet<MacAddress> = HashSet::default();
        let read = program_data.merged_config.read().await;

        // check rules
        // pinned device
        if read.args.pin_device {
            if let Some(device) = self.connected_device.read().await.as_ref() {
                filter.insert(device.get_addr());
            }
        }

        // device not pinned
        if filter.is_empty() {
            if read.args.accept_new_device {
                // check mac address
                if let Some(mac) = read.args.hrm_mac {
                    filter.insert(mac);
                }
            } else if let Some(index) = read.args.hrm_index {
                // device index
                if let Some(device) = read.program_config.hrm_list.get((index - 1) as usize) {
                    filter.insert(device.mac);
                }
            }
        }
        let filter_bdaddr: Vec<BDAddr> = filter.iter().map(|a| BDAddr::from(a.bytes())).collect();

        let known_bdaddr: Vec<BDAddr> = read
            .program_config
            .hrm_list
            .iter()
            .map(|d| BDAddr::from(d.mac.bytes()))
            .collect();
        drop(read);

        let mut found: Vec<FoundDevice> = vec![];

        let manager = Manager::new().await?;
        let adapter_list = manager.adapters().await?;
        if adapter_list.is_empty() {
            return Err(anyhow!("No Bluetooth adapters found"));
        }

        for adapter in &adapter_list {
            info!("Starting scan for devices...");
            adapter
                .start_scan(ScanFilter::default())
                .await?;
            time::sleep(Duration::from_secs(2)).await;
            let peripherals = adapter.peripherals().await?;
            if peripherals.is_empty() {
                warn!("Did not find any devices (unfiltered). Make sure your device is visible!");
                continue;
            }

            // All peripheral devices in range.
            for peripheral in &peripherals {
                let Some(properties) = peripheral.properties().await? else {
                    debug!("An unknown device does not have properties and will be skipped");
                    continue;
                };
                let clone = properties.clone();
                let local_name = properties
                    .local_name
                    .unwrap_or(String::from("[peripheral name unknown]"));

                found.push(
                    FoundDevice {
                        name: local_name,
                        addr: peripheral.address(),
                        peripheral: peripheral.clone(),
                        is_known: known_bdaddr.contains(&peripheral.address()),
                        filtered: filter_bdaddr.contains(&peripheral.address()),
                        properties: clone,
                    }
                );
            }

            if !found.is_empty() {
                return Ok(
                    found
                        .into_iter()
                        .sorted_by_key(
                            |f| (
                                filter_bdaddr.iter()
                                    .position(|add| add == &f.addr)
                                    .unwrap_or(filter_bdaddr.len()
                                    ),
                                known_bdaddr.iter()
                                    .position(|add| add == &f.addr)
                                    .unwrap_or(known_bdaddr.len()
                                    ),
                                f.name.clone().make_ascii_lowercase()
                            )
                        )
                        .collect()
                );
            }
        }

        Ok(vec![])
    }

    async fn choose_device(
        &self,
        devices: Vec<FoundDevice>,
        is_reconnect: bool,
        program_data: &Arc<ProgramData>,
    ) -> Option<FoundDevice> {
        let read = program_data.merged_config.read().await;

        // device found automatically
        let first = devices.first()?;
        if read.args.accept_new_device {
            let first_device = first;
            if first_device.filtered {
                return Some(first_device.clone());
            }
        } else {
            let first_device = first;
            if first_device.is_known {
                return Some(first_device.clone());
            }
        }
        drop(read);

        // choose manually
        let mut first_run = true;
        loop {
            let timeout_hint;
            let timeout;
            if program_data.merged_config.read().await.args.noninteractive_rescan && is_reconnect {
                timeout = Some(Duration::from_secs(1));
                timeout_hint = " (timeout 1 sec)";
            } else {
                timeout = None;
                timeout_hint = "";
            };

            if first_run {
                println!("Device to connect to could not be determined automatically. Please select{timeout_hint}:");
                first_run = false;
            } else {
                println!("Please select{timeout_hint}:");
            }

            println!("A number between 1 and {} or \"r\" to trigger a rescan.", devices.len());
            println!("{0: <10} | {1: <30} | {2: <10}", "Index", "Name", "Mac Address");
            for (i, device) in devices.iter().enumerate() {
                println!("{0: <10} | {1: <30} | {2: <10}", i + 1, device.name.chars().take(30).collect::<String>(), device.addr);
            }
            print!("Choose: ");

            let _ = io::stdout().flush();
            match next_line(true, timeout).await {
                None => {
                    return None;
                }
                Some(line) => {
                    if line == "r" {
                        return None;
                    } else if let Ok(number) = line.parse::<u8>() {
                        #[allow(clippy::cast_possible_truncation)]
                        if !(0 < number && number <= devices.len() as u8) {
                            println!("Invalid input, please try again!");
                            continue;
                        }
                        return devices.get((number - 1) as usize).cloned();
                    }
                    println!("Invalid input, please try again!");
                    continue;
                }
            }
        }
    }
}
