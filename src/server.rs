use crate::rpc::rpc_handler::handle_connection;
use log::{error, info, warn};
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, TrySendError};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::{env, thread};
use tiny_http::{Server, Response};
use crate::safe_db::edge_db_dispenser::EdgeDbDispenser;

struct ServerState {
    is_full: Mutex<bool>,
    pending_requests: Mutex<usize>,
    thread_handles: Mutex<Vec<ThreadStatus>>,
}

struct ThreadStatus {
    is_running: Arc<AtomicBool>,
}

impl ServerState {
    fn new() -> Self {
        ServerState {
            is_full: Mutex::new(false),
            pending_requests: Mutex::new(0),
            thread_handles: Mutex::new(Vec::new()),
        }
    }
}

fn spawn_worker_threads(
    threads: u64,
    protected_receiver: Arc<Mutex<mpsc::Receiver<TcpStream>>>,
    edge_db_dispenser: Arc<EdgeDbDispenser>,
    state: Arc<ServerState>,
) {
    for _ in 0..threads {
        let rec = Arc::clone(&protected_receiver);
        let dispenser_clone = Arc::clone(&edge_db_dispenser);
        let state_clone = Arc::clone(&state);
        let is_running = Arc::new(AtomicBool::new(true));
        let is_running_clone = Arc::clone(&is_running);

        let handle = thread::spawn(move || {
            let result = std::panic::catch_unwind(|| {
                while is_running_clone.load(Ordering::SeqCst) {
                    if let Ok(socket) = rec.lock().unwrap().recv() {
                        *state_clone.pending_requests.lock().unwrap() -= 1;
                        if let Err(e) = handle_connection(&dispenser_clone, socket) {
                            error!("Error handling connection: {}", e);
                        }
                    }
                }
            });

            if result.is_err() {
                error!("Thread {:?} encountered an error and stopped.", thread::current().id());
            }

            is_running_clone.store(false, Ordering::SeqCst);
        });

        info!("Spawned thread: {:?}.", handle.thread().id());
        state.thread_handles.lock().unwrap().push(ThreadStatus { is_running });
    }
}

fn health_check_server(health_listen_at: String, queue_size: usize, state: Arc<ServerState>) {
    thread::spawn(move || {
        let health_server = Server::http(health_listen_at).expect("Could not create health check server.");
        for request in health_server.incoming_requests() {
            let is_full = *state.is_full.lock().unwrap();
            let pending = *state.pending_requests.lock().unwrap();
            let threads_running = state.thread_handles.lock().unwrap().iter().all(|status| status.is_running.load(Ordering::SeqCst));

            let status_message;
            let status_code;

            if is_full {
                status_message = "Service is full";
                status_code = 503;
            } else if pending >= queue_size {
                status_message = "Too many pending requests";
                status_code = 503;
            } else if !threads_running {
                status_message = "Degraded: Not all worker threads are running.";
                status_code = 200;
            } else {
                status_message = "OK";
                status_code = 200;
            }

            let response = Response::from_string(status_message).with_status_code(status_code);
            if let Err(e) = request.respond(response) {
                warn!("Failed to respond to health check request: {}", e);
            }
        }
    });
}

fn main_server_loop(listener: TcpListener, sender: mpsc::SyncSender<TcpStream>, state: Arc<ServerState>) {
    loop {
        match listener.accept() {
            Ok((socket, _)) => match sender.try_send(socket) {
                Ok(()) => {
                    *state.is_full.lock().unwrap() = false;
                    *state.pending_requests.lock().unwrap() += 1;
                }
                Err(TrySendError::Full(mut socket)) => {
                    *state.is_full.lock().unwrap() = true;
                    if let Err(e) = socket.write_all(b"HTTP/1.1 503 Service Unavailable\r\n\r\n") {
                        warn!("Failed to send 503 response: {}", e);
                    }
                }
                Err(TrySendError::Disconnected(_)) => {
                    panic!("Internal communication channel disconnected.");
                }
            },
            Err(e) => error!("Error accepting connection: {}", e),
        }
    }
}

pub fn start_server(listen_at: &str, health_listen_at: String, queue_size: usize, threads: u64) {
    // Check if RUST_LOG is already set, and set it to "info" if not
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "info");
    }
    env_logger::init();

    info!(
        "Starting pathfinder. Listening at {} with {} threads and queue size {}.",
        listen_at, threads, queue_size
    );

    let edge_db_dispenser = Arc::new(EdgeDbDispenser::new());
    let (sender, receiver) = mpsc::sync_channel(queue_size);
    let protected_receiver = Arc::new(Mutex::new(receiver));
    let state = Arc::new(ServerState::new());

    spawn_worker_threads(threads, Arc::clone(&protected_receiver), Arc::clone(&edge_db_dispenser), Arc::clone(&state));
    health_check_server(health_listen_at, queue_size, Arc::clone(&state));

    let listener = TcpListener::bind(listen_at).expect("Could not create server.");
    main_server_loop(listener, sender, state);
}
