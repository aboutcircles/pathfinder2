use std::env;
use pathfinder2::server;
use num_cpus;

fn main() {
    let server_listen_at = env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:8080".to_string());

    let health_listen_at = env::args()
        .nth(2)
        .unwrap_or_else(|| "127.0.0.1:8081".to_string());

    let thread_count = env::args()
        .nth(3)
        .unwrap_or_else(|| num_cpus::get().to_string())
        .parse::<u64>()
        .unwrap();

    let queue_size = env::args()
        .nth(4)
        .unwrap_or_else(|| (thread_count).to_string())
        .parse::<usize>()
        .unwrap();

    server::start_server(&server_listen_at, health_listen_at, queue_size, thread_count);
}
