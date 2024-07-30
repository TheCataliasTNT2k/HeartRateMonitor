#![allow(clippy::use_debug)]

use std::str::from_utf8;
use std::sync::Arc;
use std::time::Duration;
use anyhow::anyhow;
use async_trait::async_trait;
use btleplug::api::{CharPropFlags, Peripheral};
use futures::StreamExt;
use itertools::Itertools;
use log::{debug, error, info};
use mac_address::MacAddress;
use tokio::time::{sleep, timeout};
use crate::adaptors::{Adaptor, FoundDevice};
use crate::config::Hrm;


fn print_value(name: &str, data: &Vec<u8>) {
    let s = match from_utf8(data.as_slice()) {
        Ok(v) => v,
        Err(e) => &format!("Invalid UTF-8 sequence: {e}"),
    };

    println!("{name}: {s}");
}

pub(super) struct AdaptorDebug {
    found_device: FoundDevice
}

#[async_trait]
impl Adaptor for AdaptorDebug {
    async fn to_hrm(&self) -> Hrm {
        Hrm {
            name: self.found_device.name.clone(),
            mac: MacAddress::from(self.found_device.addr.into_inner()),
            adaptor_id: Some(0),
        }
    }

    fn get_addr(&self) -> MacAddress {
        MacAddress::from(self.found_device.addr.into_inner())
    }

    async fn shutdown(&self) {
        let _ = self.found_device.peripheral.disconnect().await;
    }

    #[allow(clippy::too_many_lines)]
    async fn heartbeat_loop(&self) -> anyhow::Result<()> {
        let device = &self.found_device;
        device.peripheral.discover_services().await?;
        let mut chars = vec![];
        debug!("Subscribing to all characteristics");
        println!("Device description:");
        println!("Name: {}", device.name);
        println!("Address: {}", device.addr);
        println!("Address Type: {:?}", device.properties.address_type);
        println!("TX Power Level: {:?}", device.properties.tx_power_level);
        println!("RSSI: {:?}", device.properties.rssi);
        println!("CLASS: {:?}", device.properties.class);
        println!("Manufacturer Data:");
        for (x, y) in &device.properties.manufacturer_data {
            println!("  {x}: {y:?}");
        }
        println!("Services:");
        for service in device.peripheral.services().iter().sorted_by_key(|s| s.uuid.to_string()) {
            println!("  {}: Primary? {}", service.uuid, service.primary);
            if service.characteristics.is_empty() {
                continue;
            }
            println!("    Characteristics:");
            for char in &service.characteristics {
                println!("      {}:\n        Flags: {:?}", char.uuid, char.properties);
                if !char.descriptors.is_empty() {
                    println!("        Descriptors:");
                    for d in &char.descriptors {
                        println!("            {d}");
                    }
                }
                if char.properties.contains(CharPropFlags::NOTIFY) {
                    device.peripheral.subscribe(char).await?;
                    chars.push(char.clone());
                }
                if char.properties.contains(CharPropFlags::READ) { 
                    let val = device.peripheral.read(char).await?;
                    println!("        Value: {val:?}");
                    if service.uuid.as_u128() == 0x0000180a_0000_1000_8000_00805f9b34fb {
                        match char.uuid.as_u128() { 
                            0x00002a23_0000_1000_8000_00805f9b34fb => {
                                print_value("          System ID", &val);
                            }
                            0x00002a24_0000_1000_8000_00805f9b34fb => {
                                print_value("          Model Number", &val);
                            }
                            0x00002a25_0000_1000_8000_00805f9b34fb => {
                                print_value("          Serial Number", &val);
                            }
                            0x00002a26_0000_1000_8000_00805f9b34fb => {
                                print_value("          Firmware Revision", &val);
                            }
                            0x00002a27_0000_1000_8000_00805f9b34fb => {
                                print_value("          Hardware Revision", &val);
                            }
                            0x00002a28_0000_1000_8000_00805f9b34fb => {
                                print_value("          Software Revision", &val);
                            }
                            0x00002a29_0000_1000_8000_00805f9b34fb => {
                                print_value("          Manufacture Name", &val);
                            }
                            0x00002a2a_0000_1000_8000_00805f9b34fb | 0x00002a50_0000_1000_8000_00805f9b34fb => {
                            }
                            _ => {
                                println!("        Unknown characteristic!");
                            }
                        }
                    }
                }
            }
            println!();
        }

        let mut notification_stream = device.peripheral.notifications().await?;
        info!("Device ready!");
        let handle = tokio::spawn(async move {
            // Process while the BLE connection is not broken or stopped.
            while let Some(data) = notification_stream.next().await {
                println!(
                    "Received data from [{:?}]: {:?}",
                    data.uuid, data.value
                );
            }
        });
        loop {
            sleep(Duration::from_secs(1)).await;
            debug!("Testing connectivity...");
            match device.peripheral.is_connected().await {
                Ok(c) => {
                    debug!("Connectivity successful!");
                    if c {
                        continue;
                    }
                    debug!("Disconnected...");
                }
                Err(err) => {
                    error!("Checking connection returned error: {err}");
                }
            }
            debug!("Reconnecting...");
            if let Ok(value) = timeout(Duration::from_secs(2), device.peripheral.connect()).await {
                match value {
                    Ok(()) => {
                        debug!("Reconnected!");
                        continue;
                    }
                    Err(err) => {
                        error!("Reconnecting returned error: {err}");
                    }
                }
            }
            error!("Timeout while reconnecting to device!");
            handle.abort();
            for char in chars  {
                device.peripheral.unsubscribe(&char).await?;
            }
            info!("Disconnecting from peripheral {:?}...", device.name);
            device.peripheral.disconnect().await?;
            return Ok(());
        }
    }

    async fn try_wrap(device: Arc<FoundDevice>) -> anyhow::Result<Option<Arc<dyn Adaptor>>>
    where
        Self: Sized
    {
        debug!("Trying debug adaptor as matcher...");

        if !device.peripheral.is_connected().await.unwrap_or(false) {
            info!("Trying to connect to {:?}...", device.name);
            if let Err(err) = device.peripheral.connect().await {
                return Err(anyhow!("Could not connect to {} because of {:?}!", device.name, err));
            }
        }
        if !device.peripheral.is_connected().await.unwrap_or(false) {
            return Err(anyhow!("Connection to {} failed; check, that your device is not connected to another host!", device.name));
        }

        debug!("debug adaptor matched device!");
        return Ok(Some(Arc::new(Self {
            found_device: (*device).clone()
        })));
    }
}