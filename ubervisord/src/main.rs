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
mod config;

use clap::{App, Arg};
use config::{Config, DaemonConfig, GlobalConfig};
use hyper::{
    body::Sender,
    service::{make_service_fn, service_fn},
    Body, Request, Response, Server,
};
use hyperlocal::UnixServerExt;
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::signal::unix::{signal, SignalKind};
use ubervisor_core::{DaemonDeathInfo, DaemonStatus};

enum DaemonState {
    NotStarted,
    Running { process: Child, start_time: Instant },
    Exited { info: DaemonDeathInfo },
}

impl From<DaemonDeathInfo> for DaemonState {
    fn from(info: DaemonDeathInfo) -> Self {
        DaemonState::Exited { info }
    }
}

impl From<&DaemonState> for DaemonStatus {
    fn from(state: &DaemonState) -> Self {
        match state {
            DaemonState::NotStarted => DaemonStatus::Stopped,
            DaemonState::Running { start_time, .. } => DaemonStatus::Running {
                uptime: start_time.elapsed().as_secs(),
            },
            DaemonState::Exited { info } => DaemonStatus::Exited { info: info.clone() },
        }
    }
}

struct Daemon {
    config: DaemonConfig,
    state: DaemonState,
}

impl Daemon {
    fn get_child_process_mut(&mut self) -> Option<&mut Child> {
        match &mut self.state {
            DaemonState::Running { process, .. } => Some(process),
            _ => None,
        }
    }
}

impl From<DaemonConfig> for Daemon {
    fn from(config: DaemonConfig) -> Self {
        Daemon {
            config: config,
            state: DaemonState::NotStarted,
        }
    }
}

struct UberContext {
    global_config: GlobalConfig,
    event_subscribers: Vec<Sender>,
    daemons: HashMap<String, Daemon>,
}

type UberContextRef = Arc<Mutex<UberContext>>;

impl UberContext {
    fn new(config: Config) -> Self {
        let Config { global, programs } = config;
        let daemons = programs.into_iter().map(|(k, v)| (k, v.into())).collect();

        Self {
            global_config: global,
            daemons,
            event_subscribers: Vec::new(),
        }
    }

    fn new_ref(config: Config) -> UberContextRef {
        Arc::new(Mutex::new(Self::new(config)))
    }

    fn get_daemon_mut(
        &mut self,
        daemon_name: &str,
    ) -> Result<&mut Daemon, ubervisor_core::RequestError> {
        self.daemons
            .get_mut(daemon_name)
            .ok_or(ubervisor_core::RequestError::UnknownDaemonName)
    }

    async fn broadcast<T: Into<hyper::body::Bytes>>(&mut self, message: T) {
        let mut closed_channels = vec![];
        let message = message.into();

        for (index, sender) in self.event_subscribers.iter_mut().enumerate() {
            if let Err(e) = sender.send_data(message.clone()).await {
                log::error!("Failed writing to channel: {:?}", e);
                closed_channels.push(index);
            }
        }

        self.event_subscribers = std::mem::take(&mut self.event_subscribers)
            .into_iter()
            .enumerate()
            .filter_map(|(i, sender)| {
                if closed_channels.contains(&i) {
                    None
                } else {
                    Some(sender)
                }
            })
            .collect();
    }

    fn add_sender(&mut self, sender: Sender) {
        self.event_subscribers.push(sender);
    }

    fn start_process(&mut self, process_name: &str) -> Result<(), ubervisor_core::RequestError> {
        let program = self.get_daemon_mut(process_name)?;

        if let DaemonState::Running { process, .. } = &mut program.state {
            match process.try_wait() {
                Ok(None) => {
                    log::debug!("Process {} already started", process_name);
                    return Err(ubervisor_core::RequestError::DaemonAlreadyStarted);
                }
                Ok(Some(exit_code)) => {
                    log::debug!(
                        "Previous instance of {} exited with {:?} (signal {:?})",
                        process_name,
                        exit_code.code(),
                        exit_code.signal()
                    )
                }
                Err(e) => {
                    log::warn!("Failed to wait on previous process instance {:?}", e);
                }
            }
        }

        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(&*program.config.command)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        if let Some(out_file) = &program.config.stdout {
            let stdout = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(out_file)
                .unwrap();
            cmd.stdout(stdout);
        }

        if let Some(out_file) = &program.config.stderr {
            let stderr = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(out_file)
                .unwrap();
            cmd.stderr(stderr);
        }

        let child = cmd
            .spawn()
            .map_err(|e| ubervisor_core::RequestError::FailedToStartDaemon(e.to_string()))?;
        log::info!("Program {} started with PID {}", process_name, child.id());
        program.state = DaemonState::Running {
            process: child,
            start_time: Instant::now(),
        };

        Ok(())
    }

    fn stop_process(&mut self, process_name: &str) -> Result<(), ubervisor_core::RequestError> {
        let program = self
            .daemons
            .get_mut(process_name)
            .ok_or(ubervisor_core::RequestError::UnknownDaemonName)?;

        let process = program
            .get_child_process_mut()
            .ok_or(ubervisor_core::RequestError::DaemonAlreadyStopped)?;

        match process.try_wait() {
            Ok(None) => {}
            Ok(Some(exit_code)) => {
                log::debug!(
                    "Previous instance of {} exited with {:?} (signal {:?})",
                    process_name,
                    exit_code.code(),
                    exit_code.signal()
                );
                return Err(ubervisor_core::RequestError::DaemonAlreadyStopped);
            }
            Err(e) => {
                log::warn!("Failed to wait on previous process instance {:?}", e);
            }
        }

        process.kill();

        Ok(())
    }

    fn get_status(&self) -> String {
        let mut status = ubervisor_core::UbervisorStatus {
            daemons: HashMap::new(),
        };
        for (name, program) in self.daemons.iter() {
            let mut daemon_status = (&program.state).into();

            status.daemons.insert(name.clone(), daemon_status);
        }
        return serde_json::to_string(&status).unwrap();
    }

    fn reap_single_child(daemon_name: &str, daemon: &mut Daemon) -> Option<DaemonDeathInfo> {
        let process = daemon.get_child_process_mut()?;

        let pid = process.id();
        match process.try_wait() {
            Err(e) => {
                log::warn!(
                    "Failed to wait on previous process instance of {} {:?}",
                    daemon_name,
                    e
                );
                None
            }
            Ok(None) => {
                // program is still running, ignore
                None
            }
            Ok(Some(exit_code)) => {
                log::debug!(
                    "Previous instance of {} exited with {:?} (signal {:?})",
                    daemon_name,
                    exit_code.code(),
                    exit_code.signal()
                );
                Some(DaemonDeathInfo {
                    pid: pid,
                    timestamp: 0,
                    exit_status: exit_code.code(),
                    signal: exit_code.signal(),
                })
            }
        }
    }

    async fn reap_children(&mut self) {
        let mut events = Vec::new();
        for (process_name, program) in self.daemons.iter_mut() {
            if let Some(death_info) = Self::reap_single_child(&process_name, program) {
                events.push(ubervisor_core::Event::DaemonStopped {
                    name: process_name.to_string(),
                    info: death_info.clone(),
                });

                program.state = DaemonState::Exited { info: death_info };
            }
        }

        for event in events.into_iter() {
            let json_string = serde_json::to_string(&event);
            let json_string = match json_string {
                Err(e) => {
                    continue;
                }
                Ok(s) => s,
            };
            self.broadcast(json_string).await;
        }
    }
}

fn make_err_response(message: &str) -> Response<Body> {
    Response::builder()
        .status(400)
        .body(Body::from(message.to_owned()))
        .unwrap()
}

async fn process_request(
    ctx: UberContextRef,
    req: Request<Body>,
) -> Result<Response<Body>, hyper::Error> {
    log::info!("New client connection");
    let data = hyper::body::to_bytes(req.into_body()).await?;
    let request = serde_json::from_slice::<ubervisor_core::Request>(data.as_ref());
    let request = match request {
        Ok(r) => r,
        Err(e) => {
            log::error!(
                "Failed to parse request: {:?} {}",
                e,
                String::from_utf8_lossy(data.as_ref())
            );
            return Ok(make_err_response("parse"));
        }
    };

    let mut ctx_handle = ctx.lock().unwrap();
    let err = match request {
        ubervisor_core::Request::Monitor => {
            log::info!("Client entered monitor mode");
            let (sender, receiver) = Body::channel();
            ctx_handle.add_sender(sender);
            return Ok(Response::new(receiver));
        }

        ubervisor_core::Request::StartDaemon { daemon } => {
            log::debug!("Client asked to start '{}'", daemon);
            ctx_handle.start_process(&*daemon).map(|_| "".into())
        }

        ubervisor_core::Request::StopDaemon { daemon } => {
            log::debug!("Client asked to stop '{}'", daemon);
            ctx_handle.stop_process(&*daemon).map(|_| "".into())
        }

        ubervisor_core::Request::Status => Ok(ctx_handle.get_status()),

        _ => return Ok(make_err_response("unimplemented")),
    };

    let (status, body) = match err {
        Ok(body) => (200, body),
        Err(resp) => (400, serde_json::to_string(&resp).unwrap()),
    };

    Ok(Response::builder()
        .status(status)
        .body(Body::from(body))
        .unwrap())
}

async fn program(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let mut user_stream = signal(SignalKind::user_defined1())?;
    let mut child_stream = signal(SignalKind::child())?;
    let mut term_stream = signal(SignalKind::terminate())?;

    let path = Path::new(&config.global.control_socket_path);

    if path.exists() {
        std::fs::remove_file(path)?;
    }

    let server = Server::bind_unix(path)?;

    let ctx = UberContext::new_ref(config);

    let service_ctx = ctx.clone();
    let make_service = make_service_fn(move |_| {
        let ctx = service_ctx.clone();
        async move { Ok::<_, hyper::Error>(service_fn(move |req| process_request(ctx.clone(), req))) }
    });

    let server = server.serve(make_service);
    let server_join_handle = tokio::spawn(server);

    let mut keep_going = true;
    while keep_going {
        tokio::select! {
            _ = user_stream.recv() => {
                log::info!("Got user signal");
                let data = &b"Florp\n"[..];
                let mut ctx = ctx.lock().unwrap();
                ctx.broadcast(data).await;
            }

            _ = child_stream.recv() => {
                log::info!("Got SIGCHLD");
                let mut ctx = ctx.lock().unwrap();
                ctx.reap_children().await;
            }

            _ = term_stream.recv() => {
                log::info!("Got sigterm");
                keep_going = false;
            }
        }
    }

    server_join_handle.abort();

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .init();
    log::info!("ubervisord startup");

    let matches = App::new("ubervisor")
        .version("0.0.0")
        .author("Gilad Naaman <gilad@naaman.io>")
        .about("Simple and non-functional non-init daemon manager")
        .arg(
            Arg::with_name("config")
                .short("c")
                .long("config")
                .value_name("FILE")
                .help("Sets a custom config file")
                .required(true)
                .takes_value(true),
        )
        .arg(
            Arg::with_name("print-config")
                .long("print-config")
                .help("Print loaded config to stdout and exit"),
        )
        .get_matches();

    let foo = matches.value_of("config").unwrap();
    let path = PathBuf::from(foo);

    let config_string = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Could not read config file {}: {:?}", foo, e.kind());
            std::process::exit(1);
        }
    };

    let config: Config = toml::from_str(&*config_string).unwrap();

    if matches.is_present("print-config") {
        println!("{:#?}", config);
        return Ok(());
    }

    program(config).await
}
