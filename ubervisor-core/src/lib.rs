use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "request")]
pub enum Request {
    StartDaemon { daemon: String },
    StopDaemon { daemon: String },
    RestartDaemon { daemon: String },
    Status,
    Monitor,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum RequestError {
    UnknownDaemonName,
    DaemonAlreadyStarted,
    DaemonAlreadyStopped,
    FailedToStartDaemon(String),
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Event {
    DaemonStarted { name: String, pid: i32 },
    DaemonStopped { name: String, info: DaemonDeathInfo },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DaemonDeathInfo {
    pub pid: u32,
    pub timestamp: u64,
    pub exit_status: Option<i32>,
    pub signal: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum DaemonStatus {
    Stopped,
    Running { uptime: u64 },
    Exited { info: DaemonDeathInfo },
}
#[derive(Debug, Serialize, Deserialize)]
pub struct UbervisorStatus {
    pub daemons: HashMap<String, DaemonStatus>,
}
