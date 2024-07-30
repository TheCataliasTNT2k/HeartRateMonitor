//! Provides a way to get a line from stdin entered by the user.
//! 
//! Can also be used to get the actual next line of input while ignoring previous lines.

use std::collections::{VecDeque};
use std::io;
use std::io::BufRead;
use std::sync::{Arc};
use std::time::Duration;
use lazy_static::lazy_static;
use log::error;
use tokio::sync::{RwLock};
use tokio::time::{Instant, sleep};

lazy_static!(
    /// Contains all data read from stdin.
    static ref STDIN_QUEUE: Arc<RwLock<VecDeque<String>>> = Arc::new(RwLock::new(VecDeque::new()));
);

pub fn run() {
    match tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build() {
        Ok(rt) => {
            rt.block_on(async {
                loop {
                    // read from stdin and add to buffer
                    let stdin = io::stdin().lock();
                    for line in stdin.lines().map_while(Result::ok) {
                        STDIN_QUEUE.write().await.push_front(line);
                    }
                }
            });
        }
        Err(err) => {
            error!("Could not start stdin loop because of: {err}!");
        }
    }
}

/// Gets the next line from stdin.
/// 
/// This can be an already sent line or the actually next line when `clear` is true.
/// 
/// `clear` will clear all current data in the buffer.
pub async fn next_line(clear: bool, timeout: Option<Duration>) -> Option<String> {
    if clear {
        STDIN_QUEUE.write().await.clear();
    }
    
    let start = Instant::now();
    loop {
        // try to get a string
        if let Some(line) = STDIN_QUEUE.write().await.pop_back() {
            return Some(line);
        }
        
        // if timeout, return None
        if let Some(t) = timeout {
            if start.elapsed().as_secs() >= t.as_secs() {
                return None;
            }
        }
        sleep(Duration::from_millis(1)).await;
    }
}