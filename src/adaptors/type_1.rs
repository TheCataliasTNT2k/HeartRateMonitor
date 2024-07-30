use std::mem;
use std::sync::Arc;
use std::time::Duration;
use anyhow::anyhow;
use async_trait::async_trait;
use btleplug::api::{Characteristic, CharPropFlags, Peripheral};
use chrono::Utc;
use futures::StreamExt;
use itertools::Itertools;
use log::{debug, error, info, warn};
use mac_address::MacAddress;
use tokio::sync::RwLock;
use tokio::time::{sleep, timeout};
use uuid::Uuid;
use crate::adaptors::{Adaptor, ChannelTransferObject, FoundDevice, HrData, HrmState, SENDER};
use crate::config::Hrm;

pub(super) struct Adaptor1 {
    found_device: FoundDevice,
    characteristics: Vec<Characteristic>,
    hrm_state: Arc<RwLock<HrmState>>,
    initial_battery: Option<u8>
}

#[async_trait]
impl Adaptor for Adaptor1 {
    async fn to_hrm(&self) -> Hrm {
        Hrm {
            name: self.found_device.name.clone(),
            mac: MacAddress::from(self.found_device.addr.into_inner()),
            adaptor_id: Some(1),
        }
    }

    fn get_addr(&self) -> MacAddress {
        MacAddress::from(self.found_device.addr.into_inner())
    }

    async fn shutdown(&self) {
        let _ = self.found_device.peripheral.disconnect().await;
    }

    async fn heartbeat_loop(&self) -> anyhow::Result<()> {
        let device = &self.found_device;
        debug!("Subscribing to characteristics {:?}", self.characteristics.iter().map(|c| c.uuid).join(","));
        for c in &self.characteristics {
            device.peripheral.subscribe(c).await?;
        }
        let mut notification_stream = device.peripheral.notifications().await?;
        info!("Device ready!");
        let hrm_state = Arc::clone(&self.hrm_state);
        let initial_battery = self.initial_battery.clone();
        let handle = tokio::spawn(async move {
            // Process while the BLE connection is not broken or stopped.
            while let Some(received_data) = notification_stream.next().await {
                debug!(
                    "Received data from [{:?}]: {:?}",
                    received_data.uuid, received_data.value[1]
                );
                let mut write = hrm_state.write().await;
                let state = &mut *write;
                if let HrmState::Disconnected = state {
                    let _ = mem::replace(state, HrmState::Ok(
                        HrData {
                            hr: 0,
                            contact_ok: None,
                            battery: None,
                        }
                    ));
                }
                if let HrmState::Ok(ref mut data) = state {
                    match received_data.uuid.as_u128() {
                        0x0000180d_0000_1000_8000_00805f9b34fb => {
                            data.battery = Some(received_data.value[0]);
                        }
                        0x00002a37_0000_1000_8000_00805f9b34fb => {
                            // heart rate format
                            if received_data.value[0] & 0b1 > 0 {
                                // HR is u16
                                data.hr = u16::from_le_bytes([received_data.value[1], received_data.value[2]]);
                            } else { 
                                // HR is u8
                                data.hr = u16::from(received_data.value[1]);
                            }
                            
                            // contact sensor supported
                            if received_data.value[0] & 0b100 > 0 {
                                // contact sensor is supported
                                data.contact_ok = Some(received_data.value[0] & 0b10 > 0);
                            } else {
                                // contact sensor is not supported
                                data.contact_ok = None;
                            }
                        }
                        _ => {}
                    }
                    if data.battery.is_none() { 
                        data.battery = initial_battery;
                    }
                }

                let _ = SENDER.send(ChannelTransferObject {
                    timestamp: Utc::now(),
                    hr_state: Some(state.clone()),
                });
            }
        });
        loop {
            // wait for one second
            sleep(Duration::from_secs(1)).await;
            debug!("Testing connectivity...");
            // check connection to device
            match device.peripheral.is_connected().await {
                // connection check not broken
                Ok(c) => {
                    debug!("Connectivity successful!");
                    // if device is connected
                    if c {
                        // loop again
                        continue;
                    }
                    // device connection lost
                    debug!("Disconnected...");
                }
                // checking connection returned an error
                Err(err) => {
                    error!("Checking connection returned error: {err}");
                }
            }
            
            // try to reconnect
            debug!("Reconnecting...");
            // give the device to seconds for reconnection
            if let Ok(value) = timeout(Duration::from_secs(2), device.peripheral.connect()).await {
                match value {
                    // connection successful
                    Ok(()) => {
                        debug!("Reconnected!");
                        continue;
                    }
                    // connection got an error
                    Err(err) => {
                        error!("Reconnecting returned error: {err}");
                    }
                }
            }
            error!("Timeout while reconnecting to device!");
            
            // kill loop, which handles heart rate events
            handle.abort();
            
            // deactivate all events
            for c in &self.characteristics {
                device.peripheral.unsubscribe(c).await?;
            }
            
            // tell the api, that we are not connected anymore
            *self.hrm_state.write().await = HrmState::Disconnected;
            let _ = SENDER.send(ChannelTransferObject {
                timestamp: Utc::now(),
                hr_state: Some(HrmState::Disconnected)
            });
            
            // disconnect properly
            info!("Disconnecting from peripheral {:?}...", device.name);
            device.peripheral.disconnect().await?;
            
            // tell the rest of program, that we disconnected (the program assumes, that this function is never finished)
            return Ok(());
        }
    }

    async fn try_wrap(device: Arc<FoundDevice>) -> anyhow::Result<Option<Arc<dyn Adaptor>>>
    where
        Self: Sized
    {
        debug!("Trying adaptor1 as matcher...");
        let mut characteristics = vec![];

        if !device.peripheral.is_connected().await.unwrap_or(false) {
            info!("Trying to connect to {:?}...", device.name);
            if let Err(err) = device.peripheral.connect().await {
                return Err(anyhow!("Could not connect to {} because of {:?}!", device.name, err));
            }
        }
        if !device.peripheral.is_connected().await.unwrap_or(false) {
            return Err(anyhow!("Connection to {} failed; check, that your device is not connected to another host!", device.name));
        }

        debug!("Discover peripheral {:?} services...", device.name);
        device.peripheral.discover_services().await?;
        if !device.properties.services.contains(&Uuid::from_u128(0x0000180d_0000_1000_8000_00805f9b34fb)) {
            return Ok(None);
        }
        debug!("Services contains correct service.");

        let mut initial_battery = None;
        if let Some(char) = device.peripheral.characteristics().iter().find(|c| c.uuid == Uuid::from_u128(0x00002a19_0000_1000_8000_00805f9b34fb)) {
            match device.peripheral.read(char).await {
                Ok(v) => {
                    info!("Device has {}% battery left!", v[0]);
                    initial_battery = Some(v[0]);
                    characteristics.push(char.clone());
                }
                Err(err) => {
                    warn!("Error while reading battery value: {err}");
                }
            }
        }

        for characteristic in device.peripheral.characteristics() {
            debug!("Checking characteristic {:?}", characteristic);
            if characteristic.uuid != Uuid::from_u128(0x00002a37_0000_1000_8000_00805f9b34fb) || !characteristic.properties.contains(CharPropFlags::NOTIFY) {
                continue;
            }
            characteristics.push(characteristic.clone());

            debug!("adaptor1 matched device!");
            return Ok(Some(Arc::new(Self {
                found_device: (*device).clone(),
                characteristics,
                hrm_state: Arc::default(),
                initial_battery
            })));
        }
        Ok(None)
    }
}