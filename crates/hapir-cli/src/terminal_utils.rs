use std::sync::atomic::{AtomicBool, Ordering};

static LOGGING_SUPPRESSED: AtomicBool = AtomicBool::new(false);

/// Suppress tracing output (used when a local agent takes over the terminal).
pub fn set_logging_suppressed(suppressed: bool) {
    LOGGING_SUPPRESSED.store(suppressed, Ordering::Relaxed);
}

/// Check whether tracing output is currently suppressed.
pub fn is_logging_suppressed() -> bool {
    LOGGING_SUPPRESSED.load(Ordering::Relaxed)
}

/// Clear the terminal screen and move cursor to top-left.
pub fn clear_screen() {
    print!("\x1b[2J\x1b[H");
    let _ = std::io::Write::flush(&mut std::io::stdout());
}

/// Prepare the terminal for a local agent process:
/// clears the screen and suppresses hapir's own log output.
pub fn prepare_for_local_agent() {
    set_logging_suppressed(true);
    clear_screen();
}

/// Restore normal terminal state after a local agent exits.
pub fn restore_after_local_agent() {
    set_logging_suppressed(false);
}

/// Display a banner indicating the session is in remote mode.
pub fn show_remote_banner() {
    use std::io::Write;
    set_logging_suppressed(true);
    clear_screen();
    let banner = "\x1b[1;36m\
╔══════════════════════════════════════════════════╗\n\
║         Session switched to Remote Mode          ║\n\
║                                                  ║\n\
║   Web UI is now controlling this session.        ║\n\
║   Press [Space] to switch back to Local Mode.    ║\n\
╚══════════════════════════════════════════════════╝\
\x1b[0m\n";
    print!("{}", banner);
    let _ = std::io::stdout().flush();
}

/// Clear the remote mode banner and restore terminal.
pub fn clear_remote_banner() {
    set_logging_suppressed(false);
    clear_screen();
}

/// Wait for the user to press the Space key.
///
/// Puts the terminal into raw mode to capture individual key presses,
/// then restores the original mode before returning.
pub async fn wait_for_space_key() {
    tokio::task::spawn_blocking(|| {
        wait_for_space_key_blocking();
    })
    .await
    .ok();
}

#[cfg(unix)]
fn wait_for_space_key_blocking() {
    use std::io::Read;
    use std::os::unix::io::AsRawFd;

    let stdin_fd = std::io::stdin().as_raw_fd();

    // Save original termios
    let mut original: libc::termios = unsafe { std::mem::zeroed() };
    unsafe {
        libc::tcgetattr(stdin_fd, &mut original);
    }

    // Set raw mode
    let mut raw = original;
    raw.c_lflag &= !(libc::ICANON | libc::ECHO);
    raw.c_cc[libc::VMIN] = 1;
    raw.c_cc[libc::VTIME] = 0;
    unsafe {
        libc::tcsetattr(stdin_fd, libc::TCSANOW, &raw);
    }

    // Read until space
    let mut buf = [0u8; 1];
    loop {
        if std::io::stdin().read_exact(&mut buf).is_ok() && buf[0] == b' ' {
            break;
        }
    }

    // Restore original termios
    unsafe {
        libc::tcsetattr(stdin_fd, libc::TCSANOW, &original);
    }
}

#[cfg(windows)]
fn wait_for_space_key_blocking() {
    use std::io::Read;

    // On Windows, we read from stdin byte by byte.
    // When running in a console, this works with the default console mode.
    let mut buf = [0u8; 1];
    loop {
        if std::io::stdin().read_exact(&mut buf).is_ok() && buf[0] == b' ' {
            break;
        }
    }
}
