use crate::rpc::rpc_handler::handle_connection;
use std::io::Write;
use std::net::TcpListener;
use std::sync::mpsc::TrySendError;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use tiny_http::{Server, Response};

use crate::safe_db::edge_db_dispenser::EdgeDbDispenser;

pub fn start_server(listen_at: &str, health_listen_at: String, queue_size: usize, threads: u64) {
    println!(
        "Starting pathfinder. Listening at {} with {} threads and queue size {}.",
        listen_at, threads, queue_size
    );

    // Create a shared EdgeDbDispenser instance
    let edge_db_dispenser: Arc<EdgeDbDispenser> = Arc::new(EdgeDbDispenser::new());

    // Create a synchronous channel with the specified queue size
    let (sender, receiver) = mpsc::sync_channel(queue_size);
    let protected_receiver = Arc::new(Mutex::new(receiver));

    // Flags to indicate if the server is full and to track pending requests
    let is_full = Arc::new(Mutex::new(false));
    let pending_requests = Arc::new(Mutex::new(0));

    // Spawn worker threads to handle incoming connections
    for _ in 0..threads {
        let rec = protected_receiver.clone();
        let dispenser_clone = Arc::clone(&edge_db_dispenser);
        let pending_requests = Arc::clone(&pending_requests);
        let t = thread::spawn(move || loop {
            let socket = rec.lock().unwrap().recv().unwrap();
            {
                // Decrement pending requests count
                let mut pending = pending_requests.lock().unwrap();
                *pending -= 1;
            }
            // Handle the connection
            if let Err(e) = handle_connection(&dispenser_clone, socket) {
                println!("Error handling connection: {}", e);
            }
        });
        println!("Spawned thread: {:?}.", t.thread().id());
    }

    // Create a separate thread for the health check server
    let health_check_is_full = Arc::clone(&is_full);
    let health_check_pending_requests = Arc::clone(&pending_requests);
    let health_listen_at_clone = health_listen_at.clone();
    thread::spawn(move || {
        // Health check server listening on port 8080
        let health_server = Server::http(health_listen_at_clone).expect("Could not create health check server.");
        for request in health_server.incoming_requests() {
            if request.url() == "/health" {
                let is_full = *health_check_is_full.lock().unwrap();
                let pending = *health_check_pending_requests.lock().unwrap();
                // Respond based on the server's current status
                if is_full || pending >= queue_size {
                    let response = Response::from_string("Service Unavailable").with_status_code(503);
                    request.respond(response).unwrap();
                } else {
                    let response = Response::from_string("OK").with_status_code(200);
                    request.respond(response).unwrap();
                }
            } else {
                // Handle other endpoints
                let response = Response::from_string("Not Found").with_status_code(404);
                request.respond(response).unwrap();
            }
        }
    });

    // Main server loop to accept incoming connections
    let listener = TcpListener::bind(listen_at).expect("Could not create server.");
    loop {
        match listener.accept() {
            Ok((socket, _)) => match sender.try_send(socket) {
                Ok(()) => {
                    // Update status and increment pending requests count
                    let mut is_full = is_full.lock().unwrap();
                    *is_full = false;
                    let mut pending = pending_requests.lock().unwrap();
                    *pending += 1;
                }
                Err(TrySendError::Full(mut socket)) => {
                    // Update status to full and respond with 503
                    let mut is_full = is_full.lock().unwrap();
                    *is_full = true;
                    let _ = socket.write_all(b"HTTP/1.1 503 Service Unavailable\r\n\r\n");
                }
                Err(TrySendError::Disconnected(_)) => {
                    panic!("Internal communication channel disconnected.");
                }
            },
            Err(e) => println!("Error accepting connection: {}", e),
        }
    }
}
