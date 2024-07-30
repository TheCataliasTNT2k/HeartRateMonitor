use std::{future::Future, pin::Pin, sync::LazyLock};
use std::collections::HashMap;
use std::sync::Arc;
use async_trait::async_trait;
use btleplug::api::{BDAddr, PeripheralProperties};
use btleplug::platform::Peripheral;
use chrono::{DateTime, Utc};
use mac_address::MacAddress;
use serde::Serialize;
use tokio::sync::broadcast::{channel, Receiver, Sender};

use crate::config::Hrm;
use crate::ProgramData;

use anyhow::Result;

pub mod type_1;
pub mod hrm;
mod adaptor_debug;

static ADAPTORS: LazyLock<HashMap<u16, GetAdaptorFn>> = LazyLock::new(|| HashMap::from([
    (1_u16, Box::new(type_1::Adaptor1::try_wrap) as _)
]));

// subscribe to this to get updates on HR data
pub static SENDER: LazyLock<Sender<ChannelTransferObject>> = LazyLock::new(|| channel::<ChannelTransferObject>(256).0);


pub type BoxFuture<T> = Pin<Box<dyn Future<Output=T> + Send>>;
type GetAdaptorFn = Box<dyn Fn(Arc<FoundDevice>) -> BoxFuture<Result<Option<Arc<dyn Adaptor>>>> + Send + Sync>;

/// use this to get a receiver for `SENDER`, which notifies you about new data
pub fn get_receiver() -> Receiver<ChannelTransferObject> {
    SENDER.subscribe()
}

/// contains update data sent through the channel for all receivers
#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ChannelTransferObject {
    pub timestamp: DateTime<Utc>,
    pub hr_state: Option<HrmState>,
}

/// state of the worn herat rate monitor
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct HrData {
    pub hr: u16,
    pub contact_ok: Option<bool>,
    pub battery: Option<u8>
}


/// state of the worn herat rate monitor
#[derive(Default, Debug, Serialize, Clone)]
#[serde(rename_all = "snake_case")]
pub enum HrmState {
    #[default]
    Disconnected,
    Ok(HrData),
}

impl HrmState {
    /// poll the channel from above and put the values in the Data struct accessible to poem
    pub fn storage_loop(data: Arc<ProgramData>) {
        tokio::spawn(async move {
            let mut receiver = SENDER.subscribe();
            loop {
                if let Ok(received) = receiver.recv().await {
                    *data.hr_data.write().await = received;
                }
            }
        });
    }
}


#[derive(Clone)]
struct FoundDevice {
    pub name: String,
    pub addr: BDAddr,
    pub peripheral: Peripheral,
    pub is_known: bool,
    pub filtered: bool,
    properties: PeripheralProperties,
}


#[async_trait]
trait Adaptor: Send + Sync {    
    async fn to_hrm(&self) -> Hrm;

    fn get_addr(&self) -> MacAddress;

    async fn shutdown(&self);

    async fn heartbeat_loop(&self) -> Result<()>;

    /// This should ONLY return an error, if it is a real error! It will cancel all other matching attempts!
    async fn try_wrap(device: Arc<FoundDevice>) -> Result<Option<Arc<dyn Adaptor>>>
    where
        Self: Sized;
}

async fn find_matching_adaptor(found_device: &FoundDevice, hrm_opt: Option<&Hrm>) -> Result<Option<Arc<dyn Adaptor>>> {
    let arc = Arc::new(found_device.clone());
    if let Some(adaptor_matcher) = hrm_opt.and_then(
        |hrm| hrm.adaptor_id.and_then(|a| ADAPTORS.get(&a))
    ) {
        if let Some(adaptor) = adaptor_matcher(Arc::clone(&arc)).await? {
            return Ok(Some(adaptor));
        }
    }

    for (_, adaptor_matcher) in ADAPTORS.iter() {
        if let Some(adaptor) = adaptor_matcher(Arc::clone(&arc)).await? {
            return Ok(Some(adaptor));
        }
    }
    Ok(None)
}