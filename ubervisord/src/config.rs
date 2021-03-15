/// ubervisor
/// Copyright © 2021 Gilad Naaman
///
/// Permission is hereby granted, free of charge, to any person obtaining a
/// copy of this software and associated documentation files (the "Software"),
/// to deal in the Software without restriction, including without limitation
/// the rights to use, copy, modify, merge, publish, distribute, sublicense,
/// and/or sell copies of the Software, and to permit persons to whom the
/// Software is furnished to do so, subject to the following conditions:
///
/// The above copyright notice and this permission notice shall be included
/// in all copies or substantial portions of the Software.
///
/// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
/// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
/// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL
/// THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR
/// OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
/// ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
/// OTHER DEALINGS IN THE SOFTWARE.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Default path for control socket
pub const DEFAULT_SOCKET_PATH: &'static str = "/tmp/ubervisor.sock";

fn default_socket_path() -> String {
    DEFAULT_SOCKET_PATH.to_owned()
}

/// Contains general top-level configuration for the ubervisor daemon
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GlobalConfig {
    #[serde(default = "default_socket_path")]
    pub control_socket_path: String,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            control_socket_path: default_socket_path(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DaemonConfig {
    pub command: String,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default = "Default::default")]
    pub global: GlobalConfig,
    pub programs: HashMap<String, DaemonConfig>,
}

impl Config {
    pub fn new() -> Self {
        Self {
            global: GlobalConfig::default(),
            programs: HashMap::new(),
        }
    }
}
