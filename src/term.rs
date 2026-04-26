use std::fs::File;
use std::io::{IsTerminal, Read, Write};
use std::process::Command;
use std::time::Duration;

use rustix::fs::{Access, access};
use rustix::io::Errno;
use rustix::process::{Pid, test_kill_process};
use rustix::termios::{Winsize, tcgetwinsize};

pub const DEFAULT_CELL_WIDTH: u32 = 8;
pub const DEFAULT_CELL_HEIGHT: u32 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub cols: u32,
    pub rows: u32,
}

pub fn size_from_full_winsize(ws: &Winsize) -> Option<Size> {
    if ws.ws_xpixel == 0 || ws.ws_ypixel == 0 || ws.ws_col == 0 || ws.ws_row == 0 {
        return None;
    }
    Some(Size {
        pixel_width: u32::from(ws.ws_xpixel),
        pixel_height: u32::from(ws.ws_ypixel),
        cols: u32::from(ws.ws_col),
        rows: u32::from(ws.ws_row),
    })
}

pub fn get_size() -> Size {
    let mut best_cols = 0;
    let mut best_rows = 0;

    if let Ok(ws) = tcgetwinsize(std::io::stderr()) {
        if let Some(size) = size_from_full_winsize(&ws) {
            return size;
        }
        if ws.ws_col > 0 && ws.ws_row > 0 && best_cols == 0 {
            best_cols = u32::from(ws.ws_col);
            best_rows = u32::from(ws.ws_row);
        }
    }

    if let Ok(ws) = tcgetwinsize(std::io::stdout()) {
        if let Some(size) = size_from_full_winsize(&ws) {
            return size;
        }
        if ws.ws_col > 0 && ws.ws_row > 0 && best_cols == 0 {
            best_cols = u32::from(ws.ws_col);
            best_rows = u32::from(ws.ws_row);
        }
    }

    if let Ok(tty) = File::open("/dev/tty")
        && let Ok(ws) = tcgetwinsize(&tty)
    {
        if let Some(size) = size_from_full_winsize(&ws) {
            return size;
        }
        if ws.ws_col > 0 && ws.ws_row > 0 && best_cols == 0 {
            best_cols = u32::from(ws.ws_col);
            best_rows = u32::from(ws.ws_row);
        }
    }

    if best_cols > 0 && best_rows > 0 {
        return Size {
            pixel_width: best_cols * DEFAULT_CELL_WIDTH,
            pixel_height: best_rows * DEFAULT_CELL_HEIGHT,
            cols: best_cols,
            rows: best_rows,
        };
    }

    Size {
        pixel_width: 640,
        pixel_height: 384,
        cols: 80,
        rows: 24,
    }
}

pub fn tmux_socket_and_pid(value: &str) -> Option<(String, i32)> {
    let (socket, rest) = value.split_once(',')?;
    if socket.is_empty() {
        return None;
    }
    let pid_text = rest.split(',').next().unwrap_or_default();
    let pid = pid_text.parse::<i32>().ok()?;
    if pid <= 0 {
        return None;
    }
    Some((socket.to_string(), pid))
}

pub fn in_tmux() -> bool {
    let Some((socket, pid)) = tmux_socket_and_pid(&std::env::var("TMUX").unwrap_or_default())
    else {
        return false;
    };
    if access(&socket, Access::READ_OK | Access::WRITE_OK).is_err() {
        return false;
    }
    let Some(pid) = Pid::from_raw(pid) else {
        return false;
    };
    match test_kill_process(pid) {
        Ok(()) => true,
        Err(err) => err == Errno::PERM,
    }
}

pub fn enable_tmux_passthrough() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    let _handle = std::thread::spawn(move || {
        let result = Command::new("tmux")
            .args(["set", "-p", "allow-passthrough", "on"])
            .output();
        let _ = tx.send(result);
    });
    match rx.recv_timeout(Duration::from_secs(2)) {
        Ok(Ok(output)) if output.status.success() => Ok(()),
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let msg = if !stderr.is_empty() {
                stderr
            } else if !stdout.is_empty() {
                stdout
            } else {
                String::from("tmux command failed")
            };
            Err(format!("failed to enable tmux passthrough: {msg}").into())
        }
        Ok(Err(e)) => Err(format!("failed to enable tmux passthrough: {e}").into()),
        Err(_) => {
            // recv_timeout elapsed: the spawned thread (and tmux process) will
            // continue running in the background but we stop waiting for it.
            // We cannot easily kill the child here because it's owned by the
            // thread, so we simply let the thread finish on its own.
            Err("failed to enable tmux passthrough: tmux command timed out".into())
        }
    }
}

pub fn read_interactive_line(prompt: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")?;
    if !prompt.is_empty() {
        tty.write_all(prompt.as_bytes())?;
        tty.flush()?;
    }
    let mut buf = String::new();
    let mut byte = [0_u8; 1];
    loop {
        let read = tty.read(&mut byte)?;
        if read == 0 {
            break;
        }
        buf.push(byte[0] as char);
        if byte[0] == b'\n' {
            break;
        }
    }
    Ok(buf)
}

pub fn is_terminal<T: IsTerminal>(value: &T) -> bool {
    value.is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn in_tmux_requires_valid_socket() {
        temp_env::with_var_unset("TMUX", || {
            assert!(!in_tmux());
        });

        let dir = tempfile::tempdir().unwrap();
        temp_env::with_var(
            "TMUX",
            Some(format!("{},12345,0", dir.path().join("missing").display())),
            || {
                assert!(!in_tmux());
            },
        );

        let socket = dir.path().join("tmux.sock");
        fs::write(&socket, []).unwrap();
        temp_env::with_var("TMUX", Some(format!("{},1,0", socket.display())), || {
            assert!(in_tmux());
        });
    }

    #[test]
    fn tmux_socket_and_pid_parsing() {
        assert_eq!(
            tmux_socket_and_pid("/tmp/tmux/default,12345,0"),
            Some((String::from("/tmp/tmux/default"), 12345))
        );
        assert_eq!(tmux_socket_and_pid("/tmp/tmux/default"), None);
        assert_eq!(tmux_socket_and_pid("/tmp/tmux/default,nope,0"), None);
    }

    #[test]
    fn size_struct_copy() {
        let zero = Size {
            pixel_width: 0,
            pixel_height: 0,
            cols: 0,
            rows: 0,
        };
        assert_eq!(zero.pixel_width, 0);
        let size = Size {
            pixel_width: 1280,
            pixel_height: 768,
            cols: 160,
            rows: 48,
        };
        let copy = size;
        assert_eq!(copy, size);
    }

    #[test]
    fn default_constants() {
        assert_eq!(DEFAULT_CELL_WIDTH, 8);
        assert_eq!(DEFAULT_CELL_HEIGHT, 16);
    }

    #[test]
    fn get_size_returns_something() {
        let size = get_size();
        assert!(size.pixel_width > 0);
        assert!(size.pixel_height > 0);
        assert!(size.cols > 0);
        assert!(size.rows > 0);
    }

    #[test]
    fn is_terminal_stdout() {
        let _ = is_terminal(&std::io::stdout());
    }

    #[test]
    fn size_from_full_winsize_requires_all_non_zero() {
        let ws = Winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 640,
            ws_ypixel: 384,
        };
        assert_eq!(
            size_from_full_winsize(&ws),
            Some(Size {
                pixel_width: 640,
                pixel_height: 384,
                cols: 80,
                rows: 24,
            })
        );
        assert_eq!(
            size_from_full_winsize(&Winsize {
                ws_row: 24,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 384,
            }),
            None
        );
        assert_eq!(
            size_from_full_winsize(&Winsize {
                ws_row: 0,
                ws_col: 0,
                ws_xpixel: 0,
                ws_ypixel: 0,
            }),
            None
        );
    }

    #[test]
    fn cell_estimate_fallback() {
        let cols = 120;
        let rows = 40;
        let size = Size {
            pixel_width: cols * DEFAULT_CELL_WIDTH,
            pixel_height: rows * DEFAULT_CELL_HEIGHT,
            cols,
            rows,
        };
        assert_eq!(size.pixel_width, 960);
        assert_eq!(size.pixel_height, 640);
    }

    #[test]
    fn test_enable_tmux_passthrough_reports_command_failure() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let fake_tmux = dir.path().join("tmux");
        std::fs::write(
            &fake_tmux,
            "#!/bin/sh\necho 'no server running' >&2\nexit 1\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&fake_tmux).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake_tmux, perms).unwrap();

        let old_path = std::env::var_os("PATH").unwrap_or_default();
        let mut new_path = dir.path().as_os_str().to_os_string();
        new_path.push(":");
        new_path.push(old_path);

        temp_env::with_var("PATH", Some(new_path.as_os_str()), || {
            let result = enable_tmux_passthrough();

            assert!(result.is_err());
            let msg = result.unwrap_err().to_string();
            assert!(
                msg.contains("failed to enable tmux passthrough"),
                "expected error message, got: {msg}"
            );
        });
    }

    #[test]
    fn test_enable_tmux_passthrough_times_out() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let fake_tmux = dir.path().join("tmux");
        // Script sleeps longer than the 2-second timeout.
        std::fs::write(&fake_tmux, "#!/bin/sh\nsleep 10\n").unwrap();
        let mut perms = std::fs::metadata(&fake_tmux).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake_tmux, perms).unwrap();

        let old_path = std::env::var_os("PATH").unwrap_or_default();
        let mut new_path = dir.path().as_os_str().to_os_string();
        new_path.push(":");
        new_path.push(old_path);

        temp_env::with_var("PATH", Some(new_path.as_os_str()), || {
            let start = std::time::Instant::now();
            let result = enable_tmux_passthrough();
            let elapsed = start.elapsed();

            assert!(result.is_err());
            assert!(
                result.unwrap_err().to_string().contains("timed out"),
                "expected timeout error"
            );
            assert!(
                elapsed < std::time::Duration::from_secs(4),
                "should complete within 4 seconds, took {elapsed:?}"
            );
        });
    }
}
