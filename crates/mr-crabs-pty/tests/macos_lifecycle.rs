#![cfg(target_os = "macos")]

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use mr_crabs_pty::{CommandBuilder, ExitStatus, PtyConfig, PtySession, PtySize, WriteError};

fn zsh(script: &str) -> CommandBuilder {
    let mut command = CommandBuilder::new("/bin/zsh");
    command.args(["-f", "-c", script]);
    command
}

fn collect_until(output: &Receiver<Vec<u8>>, needle: &[u8], timeout: Duration) -> Vec<u8> {
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match output.recv_timeout(remaining.min(Duration::from_millis(100))) {
            Ok(chunk) => {
                bytes.extend_from_slice(&chunk);
                if bytes.windows(needle.len()).any(|window| window == needle) {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    bytes
}

fn wait_on_wake<T>(
    wake_rx: &Receiver<()>,
    timeout: Duration,
    mut check: impl FnMut() -> Option<T>,
) -> Option<T> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match wake_rx.recv_timeout(remaining) {
            Ok(()) => {
                if let Some(value) = check() {
                    return Some(value);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return check(),
        }
    }
    None
}

#[test]
fn idle_output_publishes_without_input() {
    let size = PtySize::new(80, 24, 8, 16).unwrap();
    let (wake_tx, wake_rx) = std::sync::mpsc::sync_channel(1);
    let wake = Arc::new(move || {
        let _ = wake_tx.try_send(());
    });
    let config = PtyConfig::new(zsh("printf 'wake-ready\\n'"), size).with_output_wake(wake);
    let (mut session, output, exit) = PtySession::spawn(config).unwrap();

    wake_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("reader must wake consumer after queueing output");
    let bytes = collect_until(&output, b"wake-ready", Duration::from_secs(2));
    assert!(bytes.windows(10).any(|window| window == b"wake-ready"));
    let status = exit.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(status.code, Some(0));
    assert_eq!(
        session
            .shutdown_and_reap(Duration::from_millis(100))
            .unwrap(),
        status
    );
}

#[test]
fn reader_termination_notifies_after_output_disconnect() {
    let size = PtySize::new(80, 24, 8, 16).unwrap();
    let (wake_tx, wake_rx) = std::sync::mpsc::sync_channel(8);
    let wake = Arc::new(move || {
        let _ = wake_tx.try_send(());
    });
    let config = PtyConfig::new(zsh("exit 0"), size).with_output_wake(wake);
    let (mut session, output, exit) = PtySession::spawn(config).unwrap();

    let disconnected = wait_on_wake(&wake_rx, Duration::from_secs(2), || {
        match output.try_recv() {
            Ok(_) => None,
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(()),
        }
    });
    assert!(
        disconnected.is_some(),
        "reader termination must wake after dropping the output sender"
    );

    let status = exit.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(status.code, Some(0));
    assert_eq!(
        session
            .shutdown_and_reap(Duration::from_millis(100))
            .unwrap(),
        status
    );
}

#[test]
fn exit_publication_notifies_after_status_is_queued() {
    let size = PtySize::new(80, 24, 8, 16).unwrap();
    let (wake_tx, wake_rx) = std::sync::mpsc::sync_channel(8);
    let wake = Arc::new(move || {
        let _ = wake_tx.try_send(());
    });
    let config = PtyConfig::new(zsh("exit 3"), size).with_output_wake(wake);
    let (mut session, _output, exit) = PtySession::spawn(config).unwrap();

    let status = wait_on_wake(&wake_rx, Duration::from_secs(2), || exit.try_recv().ok())
        .expect("exit publication must wake after status is queued");
    assert_eq!(status.code, Some(3));
    assert_eq!(
        session
            .shutdown_and_reap(Duration::from_millis(100))
            .unwrap(),
        status
    );
}

#[test]
fn zsh_echo_resize_and_reap_once() {
    let size = PtySize::new(80, 24, 8, 16).unwrap();
    let config = PtyConfig::new(
        zsh("printf 'ready\\n'; read line; printf 'got:%s\\n' $line; stty size"),
        size,
    );
    let (mut session, output, exit) = PtySession::spawn(config).unwrap();

    let ready = collect_until(&output, b"ready", Duration::from_secs(2));
    assert!(
        ready.windows(5).any(|window| window == b"ready"),
        "{ready:?}"
    );

    session
        .resize(PtySize::new(132, 43, 8, 16).unwrap())
        .unwrap();
    session
        .write_timeout(b"hello\n", Duration::from_secs(1))
        .unwrap();
    let response = collect_until(&output, b"43 132", Duration::from_secs(2));
    assert!(
        response.windows(9).any(|window| window == b"got:hello"),
        "{response:?}"
    );
    assert!(
        response.windows(6).any(|window| window == b"43 132"),
        "{response:?}"
    );

    let reported = exit.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(
        reported,
        ExitStatus {
            code: Some(0),
            signal: None
        }
    );
    assert_eq!(session.try_wait().unwrap(), Some(reported));
    assert_eq!(
        session
            .shutdown_and_reap(Duration::from_millis(100))
            .unwrap(),
        reported
    );
    assert_eq!(
        session
            .shutdown_and_reap(Duration::from_millis(100))
            .unwrap(),
        reported
    );
}

#[test]
fn shutdown_terminates_child_that_ignores_term() {
    let size = PtySize::new(80, 24, 8, 16).unwrap();
    let config = PtyConfig::new(
        zsh("trap '' HUP TERM; printf 'waiting\\n'; while true; do sleep 1; done"),
        size,
    );
    let (mut session, output, _exit) = PtySession::spawn(config).unwrap();
    let waiting = collect_until(&output, b"waiting", Duration::from_secs(2));
    assert!(
        waiting.windows(7).any(|window| window == b"waiting"),
        "{waiting:?}"
    );

    let started = Instant::now();
    let status = session
        .shutdown_and_reap(Duration::from_millis(50))
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(status.signal, Some(libc::SIGKILL));
}

#[test]
fn bounded_writer_backpressures_and_shutdown_stays_bounded() {
    let size = PtySize::new(80, 24, 8, 16).unwrap();
    let config = PtyConfig::new(zsh("trap '' HUP TERM; printf 'waiting\\n'; sleep 30"), size)
        .with_writer_capacity(1);
    let (mut session, output, _exit) = PtySession::spawn(config).unwrap();
    let waiting = collect_until(&output, b"waiting", Duration::from_secs(2));
    assert!(
        waiting.windows(7).any(|window| window == b"waiting"),
        "{waiting:?}"
    );
    let chunk = vec![b'x'; 1024 * 1024];

    let mut observed_full = false;
    for _ in 0..64 {
        match session.try_write(&chunk) {
            Ok(()) => {}
            Err(WriteError::Full) => {
                observed_full = true;
                break;
            }
            Err(error) => panic!("unexpected write result: {error}"),
        }
    }
    assert!(
        observed_full,
        "bounded writer queue never applied backpressure"
    );

    let started = Instant::now();
    let status = session
        .shutdown_and_reap(Duration::from_millis(50))
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(status.signal, Some(libc::SIGKILL));
}

#[test]
fn eof_closes_stdin_and_child_exits_cleanly() {
    let size = PtySize::new(80, 24, 8, 16).unwrap();
    let config = PtyConfig::new(zsh("cat; printf 'eof-seen\\n'"), size);
    let (mut session, output, exit) = PtySession::spawn(config).unwrap();

    session
        .write_timeout(b"\x04", Duration::from_secs(1))
        .unwrap();
    let response = collect_until(&output, b"eof-seen", Duration::from_secs(2));
    assert!(
        response.windows(8).any(|window| window == b"eof-seen"),
        "{response:?}"
    );
    let status = exit.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(status.code, Some(0));
    assert_eq!(
        session
            .shutdown_and_reap(Duration::from_millis(100))
            .unwrap(),
        status
    );
}

#[test]
fn child_crash_reports_signal_and_reaps() {
    let size = PtySize::new(80, 24, 8, 16).unwrap();
    let config = PtyConfig::new(zsh("kill -SEGV $$"), size);
    let (mut session, _output, exit) = PtySession::spawn(config).unwrap();

    let status = exit.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(status.signal, Some(libc::SIGSEGV));
    assert_eq!(
        session
            .shutdown_and_reap(Duration::from_millis(100))
            .unwrap(),
        status
    );
}

#[test]
fn rapid_resize_keeps_final_dimensions() {
    let size = PtySize::new(80, 24, 8, 16).unwrap();
    let config = PtyConfig::new(zsh("printf 'ready\\n'; read line; stty size"), size);
    let (mut session, output, exit) = PtySession::spawn(config).unwrap();
    let ready = collect_until(&output, b"ready", Duration::from_secs(2));
    assert!(
        ready.windows(5).any(|window| window == b"ready"),
        "{ready:?}"
    );

    for step in 1..=64 {
        session
            .resize(PtySize::new(80 + step, 24 + step, 8, 16).unwrap())
            .unwrap();
    }
    session
        .write_timeout(b"go\n", Duration::from_secs(1))
        .unwrap();
    let response = collect_until(&output, b"88 144", Duration::from_secs(2));
    assert!(
        response.windows(6).any(|window| window == b"88 144"),
        "{response:?}"
    );
    let status = exit.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(status.code, Some(0));
    assert_eq!(
        session
            .shutdown_and_reap(Duration::from_millis(100))
            .unwrap(),
        status
    );
}

#[test]
fn dropping_parent_reaps_child() {
    let size = PtySize::new(80, 24, 8, 16).unwrap();
    let config = PtyConfig::new(zsh("trap '' HUP TERM; printf 'waiting\\n'; sleep 30"), size);
    let (session, output, _exit) = PtySession::spawn(config).unwrap();
    let pid = session.child_pid();
    let waiting = collect_until(&output, b"waiting", Duration::from_secs(2));
    assert!(
        waiting.windows(7).any(|window| window == b"waiting"),
        "{waiting:?}"
    );

    drop(session);
    // SAFETY: signal 0 performs only an existence/permission check for the
    // captured child PID and does not mutate process state.
    let result = unsafe { libc::kill(pid, 0) };
    assert_eq!(result, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}
