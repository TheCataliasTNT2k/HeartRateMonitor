//! ShutdownHandler
//!
//! Waits for the program to shut down and calls predefined hooks anywhere in the program to execute cleanup tasks.
//!
//! Shutdown can be triggered by
//! - dropping the instance of this object,
//! - quitting the program via most signals the OS provides for this purpose.
//!
//! NOTES:
//! - Calling `exit()` will NOT run the shutdown sequence!
//! - The shutdown handler will NOT exit the program after finishing!
//! - The timeout for all cleanup tasks is 1 second.

use std::future::Future;
use std::mem;
use std::pin::Pin;
use std::process::exit;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use log::{debug, error, info};
#[cfg(unix)]
use tokio::signal::unix::{signal, SignalKind};
#[cfg(windows)]
use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close, ctrl_logoff, ctrl_shutdown};
use tokio::sync::RwLock;
use tokio::task::JoinSet;
use tokio::time::timeout;
use crate::CANCELLATION_TOKEN;

pub type ShutdownFunc = Box<dyn Fn() -> Pin<Box<dyn Future<Output=()> + Send>> + Send + Sync>;
type HookVec = RwLock<Vec<ShutdownFunc>>;


/// Struct to handle shutdown of the program.
pub(crate) struct ShutdownHandler {
    /// Vec of shutdown hooks to execute.
    shutdown_hooks: HookVec,
}

impl Drop for ShutdownHandler {
    fn drop(&mut self) {
        // Ensure, that everyone was notified at least once.
        CANCELLATION_TOKEN.cancel();
        info!("Calling shutdown hooks...");
        std::thread::scope(|s| {
            let _ = s.spawn(|| {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build() {
                    Ok(rt) => {
                        rt.block_on(async {
                            let mut set = JoinSet::new();

                            // get all shutdown hooks
                            let hooks = mem::take(&mut self.shutdown_hooks);
                            
                            // start all shutdown hooks concurrently
                            for shutdown_hook in hooks.into_inner() {
                                set.spawn(shutdown_hook());
                            }

                            // wait at most 1 second for everything to complete
                            let _ = timeout(
                                Duration::from_secs(1),
                                async {
                                    while set.join_next().await.is_some() {}
                                }).await;
                        });
                    }
                    Err(err) => {
                        error!("Error while running shutdown handlers: {err}!");
                    }
                }
            }).join();
        });
    }
}

impl ShutdownHandler {
    /// Creates a new shutdown handler to be used.
    ///
    /// Drop it to execute the shutdown hooks.
    pub fn new() -> Self {
        ShutdownHandler {
            shutdown_hooks: RwLock::default(),
        }
    }

    /// Add a new hook to the shutdown handler.
    ///
    /// This will NOT filter for uniqueness.
    pub async fn register_hook(&self, hook: ShutdownFunc) {
        self.shutdown_hooks.write().await.push(hook);
    }

    /// Watch OS signals to send message in channel when necessary
    pub fn create_watchers() {
        #[cfg(windows)]
        {
            macro_rules! signals {
                ($(($func:tt, $name:literal)),*) => {
                    $(
                        tokio::spawn(async move {
                            let mut stream = match $func() {
                                Ok(v) => {
                                    debug!("Registered signal handler for $name.");
                                    v
                                }
                                Err(err) => {
                                    error!("Could not register signal handler for $name because: {err}");
                                    exit(1);
                                }
                            };
                            // wait for signal to arrive
                            stream.recv().await;
                            info!("Got signal $name, shutting down.");
                            // send message in channel
                            CANCELLATION_TOKEN.cancel();
                        });
                    )*
                };
            }

            // register all signal handlers
            signals!(
                (ctrl_break, "CTRL_BREAK"),
                (ctrl_c, "CTRL_C"),
                (ctrl_close, "CTRL_CLOSE"),
                (ctrl_shutdown, "CTRL_SHUTDOWN"),
                (ctrl_logoff, "CTRL_LOGOFF")
            );
        }

        #[cfg(unix)]
        {
            // register all signals
            for (signal_kind, name) in [
                (SignalKind::interrupt as fn() -> SignalKind, "SIG_INTERRUPT"),
                (SignalKind::terminate as fn() -> SignalKind, "SIG_TERMINATE"),
                (SignalKind::quit as fn() -> SignalKind, "SIG_QUIT"),
            ] {
                tokio::spawn(async move {
                    let mut stream = match signal(signal_kind()) {
                        Ok(v) => {
                            debug!("Registered signal handler for {name}.");
                            v
                        }
                        Err(err) => {
                            error!("Could not register signal handler for {name} because: {err}");
                            exit(1);
                        }
                    };
                    // wait for signal to arrive
                    stream.recv().await;
                    info!("Got signal {name}, shutting down.");
                    // send message in channel
                    CANCELLATION_TOKEN.cancel();
                });
            }
        }
    }
}

/// Trait to implement, if the datastructure wants to register a shutdown hook. 
#[async_trait]
pub trait Shutdown {
    /// Add a shutdown hook for this data structure to be called.
    async fn register_shutdown_hook(&self, shutdown_handler: Arc<ShutdownHandler>);
}
