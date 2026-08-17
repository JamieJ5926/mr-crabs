use std::time::{Duration, Instant};

use mr_crabs_pty::{CommandBuilder, PtyConfig, PtySession, PtySize};

fn main() {
    // Production-style command: discover the login shell, NO arguments
    // (the exact path used by PaneSession::spawn_with_output_wake).
    let mut cmd = CommandBuilder::new("/bin/sh");
    cmd.shell(None);
    let size = PtySize::new(80, 24, 8, 16).expect("valid size");
    let config = PtyConfig::new(cmd, size);
    let (mut session, rx, _exit) = PtySession::spawn(config).expect("spawn");
    println!("child pid = {}", session.child_pid());
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut total = 0usize;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(chunk) => {
                total += chunk.len();
                let shown = String::from_utf8_lossy(&chunk)
                    .chars()
                    .take(120)
                    .collect::<String>();
                println!("[{} total] read {} bytes: {:?}", total, chunk.len(), shown);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                println!("DISCONNECTED");
                break;
            }
        }
    }
    println!("TOTAL BYTES = {}", total);
    let _ = session.shutdown_and_reap(Duration::from_secs(2));
}
