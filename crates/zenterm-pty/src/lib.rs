//! PTY session management.
//!
//! Wraps [`portable_pty`] to spawn a shell and provide non-blocking
//! read/write access via a background thread.

use std::io::{BufReader, Read, Write};
use std::sync::mpsc;
use std::thread;

use portable_pty::{
    Child, CommandBuilder, ExitStatus, MasterPty, NativePtySystem, PtySize, PtySystem,
};

use zenterm_core::{Error, Result, TermSize};

type DataHandler = Box<dyn Fn(&[u8]) + Send + Sync>;

const DEFAULT_TERM: &str = "xterm-256color";

fn configure_command_environment(cmd: &mut CommandBuilder) {
    // Advertise the terminal capabilities implemented by zenterm. GUI
    // launchers do not necessarily provide TERM, which leaves shells such as
    // zsh without terminfo bindings for keys like forward Delete.
    cmd.env("TERM", DEFAULT_TERM);
    cmd.env("TERM_PROGRAM", "zenterm");
}

/// A running PTY session connected to a shell process.
///
/// Ownership order in the struct is significant for [`Drop`]:
/// 1. `writer` (dropped first — sends EOF to slave)
/// 2. `rx` (channel receiver — no side-effects; dropping it causes the
///    reader thread's next `tx.send()` to fail, which makes it exit)
/// 3. `master` (dropped before the reader thread handle so that the
///    reader can detect the close on platforms where master-drop
///    unblocks the underlying fd)
/// 4. `_reader_thread` (dropped last — the handle is taken and dropped
///    in [`close()`](Self::close()) to **detach** the reader thread;
///    joining is explicitly avoided to prevent deadlock when the
///    reader is blocked on a PTY `read()` that never gets EOF)
pub struct PtySession {
    /// Writer — send keyboard input to the shell (obtained via `take_writer`).
    writer: Option<Box<dyn Write + Send>>,
    /// Receiver — bytes emitted by the shell arrive here.
    rx: mpsc::Receiver<Vec<u8>>,
    /// The master PTY handle (kept alive for resize; dropped early during
    /// [`close()`](Self::close()) to unblock the reader thread on Windows).
    master: Option<Box<dyn MasterPty>>,
    /// Handle to the background reader thread.
    _reader_thread: Option<thread::JoinHandle<()>>,
    /// Handle to the child process (shell), kept alive so we can detect
    /// when it exits (necessary on Windows ConPTY where the output pipe
    /// is *not* closed automatically on child exit).
    child: Option<Box<dyn Child + Send + Sync>>,
}

impl PtySession {
    /// Spawn a new shell in a PTY of the given size.
    ///
    /// Uses the user's default shell.  No wakeup callback — the caller
    /// must poll [`try_read()`](Self::try_read) to receive data.
    pub fn spawn(size: TermSize) -> Result<Self> {
        Self::spawn_with_wakeup(size, None)
    }

    /// Spawn a new shell with an optional wakeup callback.
    ///
    /// When provided, `wakeup` is invoked from the reader thread after
    /// each successful PTY read.  This is the primary mechanism for
    /// event-driven wakeup: the callback typically calls
    /// `egui::Context::request_repaint()` to notify the UI thread.
    ///
    /// The existing data channel (`try_read`) remains active — the
    /// wakeup is an addition, not a replacement.  If you pass `None`,
    /// behaviour is identical to [`spawn()`](Self::spawn).
    pub fn spawn_with_wakeup(
        size: TermSize,
        wakeup: Option<Box<dyn Fn() + Send + Sync>>,
    ) -> Result<Self> {
        Self::spawn_with_handlers(size, wakeup, None)
    }

    /// Spawn a new shell with wakeup and optional data handler.
    ///
    /// When `on_data` is provided, the reader thread calls this
    /// callback with each chunk of raw PTY data **instead of** sending
    /// the data through the internal channel.  This enables the reader
    /// thread to process PTY data directly (e.g. call
    /// `terminal.feed()` on a shared terminal state).
    ///
    /// Even with `on_data`, the channel still receives an empty `Vec`
    /// on EOF so that [`try_read()`](Self::try_read) can detect shell
    /// exit.
    ///
    /// When `on_data` is `None`, behaviour is identical to
    /// [`spawn_with_wakeup()`](Self::spawn_with_wakeup).
    pub fn spawn_with_handlers(
        size: TermSize,
        wakeup: Option<Box<dyn Fn() + Send + Sync>>,
        on_data: Option<DataHandler>,
    ) -> Result<Self> {
        let pty_system = NativePtySystem::default();

        let pair = pty_system
            .openpty(PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: size.pixel_width,
                pixel_height: size.pixel_height,
            })
            .map_err(|e| Error::Pty(e.to_string()))?;

        let mut cmd = CommandBuilder::new_default_prog();
        configure_command_environment(&mut cmd);
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| Error::Pty(e.to_string()))?;

        let master = pair.master;

        // Obtain reader and writer from the master end.
        let reader = master
            .try_clone_reader()
            .map_err(|e| Error::Pty(e.to_string()))?;

        let writer = master
            .take_writer()
            .map_err(|e| Error::Pty(e.to_string()))?;

        // Spawn a background thread that reads PTY bytes and sends them
        // over a bounded channel to the main thread. A blocking send gives
        // the PTY a real backpressure path: output is retained in order and
        // the shell eventually blocks on the OS PTY buffer instead of losing
        // terminal data or growing memory without bound.
        let (tx, rx) = mpsc::sync_channel(256);
        let _reader_thread = thread::Builder::new()
            .name("pty-reader".into())
            .spawn(move || {
                let mut reader = BufReader::new(reader);
                let mut buf = [0u8; 65536];
                log::trace!("pty-reader thread started");
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => {
                            // EOF — shell exited.
                            log::trace!("pty-reader: EOF from PTY");
                            let _ = tx.try_send(Vec::new());
                            break;
                        }
                        Ok(n) => {
                            log::trace!("pty-reader: read {} bytes", n);
                            if let Some(ref handler) = on_data {
                                handler(&buf[..n]);
                            } else {
                                if let Err(mpsc::SendError(_)) = tx.send(buf[..n].to_vec()) {
                                    log::trace!("pty-reader: channel closed, exiting");
                                    break;
                                }
                            }
                            // Notify the event loop that data is available.
                            // Called even when the channel was full (the
                            // main thread may have pending data to process).
                            if let Some(ref w) = wakeup {
                                w();
                            }
                        }
                        Err(e) => {
                            log::error!("pty-reader error: {e}");
                            break;
                        }
                    }
                }
            })
            .map_err(|e| Error::Pty(e.to_string()))?;

        Ok(Self {
            writer: Some(Box::new(writer)),
            rx,
            master: Some(master),
            _reader_thread: Some(_reader_thread),
            child: Some(child),
        })
    }

    /// Try to read pending bytes from the shell (non-blocking).
    ///
    /// Returns `None` if no data is available.
    /// Returns `Some(Ok(bytes))` on data.
    /// Returns `Some(Err(...))` on shell exit.
    pub fn try_read(&self) -> Option<Result<Vec<u8>>> {
        match self.rx.try_recv() {
            Ok(data) => {
                if data.is_empty() {
                    log::trace!("pty try_read: shell exited");
                    Some(Err(Error::Pty("shell exited".into())))
                } else {
                    log::trace!("pty try_read: {} bytes", data.len());
                    Some(Ok(data))
                }
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                log::trace!("pty try_read: reader thread disconnected");
                Some(Err(Error::Pty("PTY reader disconnected".into())))
            }
        }
    }

    /// Non-blocking check whether the child process has exited.
    ///
    /// Returns `Some(exit_status)` if the child has exited, or `None` if
    /// it is still running.  This is essential on **Windows ConPTY** where
    /// the output pipe is *not* automatically closed when the child exits,
    /// so the reader thread never produces an EOF and [`try_read()`] alone
    /// cannot detect shell termination.
    pub fn try_wait(&mut self) -> Option<ExitStatus> {
        self.child
            .as_mut()
            .and_then(|child| child.try_wait().ok().flatten())
    }

    /// Close the PTY session, unblocking the reader thread.
    ///
    /// 1. Drop the writer (sends EOF to the slave end).
    /// 2. Drop the master PTY handle — on Windows this calls
    ///    [`ClosePseudoConsole`], which closes the output pipes and unblocks
    ///    the reader thread; on Unix the master fd is closed.
    /// 3. Join the background reader thread (which should now have received
    ///    EOF or an error and exited).
    ///
    /// After this call the session is inert: [`try_read()`] will return
    /// `Err(Disconnected)` and [`write()`] will return `Err("writer already
    /// taken")`.  Safe to call multiple times.
    pub fn close(&mut self) {
        // 1. Drop writer (sends EOF to slave).
        self.writer.take();

        // 2. Drop master — on Windows this calls ClosePseudoConsole, which
        //    closes the output pipes and unblocks the reader thread.
        //    Order matters: master must be dropped *before* the reader
        //    thread handle is dropped, so that the reader can detect the
        //    close on any platform where master-drop unblocks the reader.
        drop(self.master.take());

        // 3. Detach the reader thread (do NOT join — the thread might still
        //    be blocked on read() if the PTY slave has open fds elsewhere,
        //    and joining would deadlock the UI thread).  The thread will
        //    exit on its own when:
        //      - it reads EOF from the PTY, or
        //      - it tries to send on the channel after rx is dropped.
        self._reader_thread.take();

        // Drop child handle (reap the zombie).
        self.child.take();
    }

    /// Write bytes to the shell's stdin.
    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        if let Some(writer) = self.writer.as_mut() {
            let start = std::time::Instant::now();
            writer.write_all(data).map_err(Error::Io)?;
            writer.flush().map_err(Error::Io)?;
            let elapsed = start.elapsed();
            if elapsed > std::time::Duration::from_millis(10) {
                log::warn!("[perf] pty::write({} bytes) took {:?}", data.len(), elapsed);
            }
            Ok(())
        } else {
            Err(Error::Pty("writer already taken".into()))
        }
    }

    /// Resize the PTY (called on window resize).
    pub fn resize(&mut self, size: TermSize) -> Result<()> {
        match self.master.as_ref() {
            Some(master) => master
                .resize(PtySize {
                    rows: size.rows,
                    cols: size.cols,
                    pixel_width: size.pixel_width,
                    pixel_height: size.pixel_height,
                })
                .map_err(|e| Error::Pty(e.to_string())),
            None => Ok(()),
        }
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::{configure_command_environment, DEFAULT_TERM};
    use portable_pty::CommandBuilder;
    use std::ffi::OsStr;

    #[test]
    fn command_environment_sets_term_when_parent_does_not() {
        let mut cmd = CommandBuilder::new_default_prog();
        cmd.env_remove("TERM");

        configure_command_environment(&mut cmd);

        assert_eq!(cmd.get_env("TERM"), Some(OsStr::new(DEFAULT_TERM)));
        assert_eq!(cmd.get_env("TERM_PROGRAM"), Some(OsStr::new("zenterm")));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_zsh_binds_forward_delete_with_default_term() {
        let output = std::process::Command::new("/bin/zsh")
            .args(["-flic", r#"source /etc/zshrc; bindkey "^[[3~""#])
            .env_remove("TERM")
            .env("TERM", DEFAULT_TERM)
            .env("TERM_PROGRAM", "zenterm")
            .output()
            .expect("failed to run zsh");

        assert!(
            output.status.success(),
            "zsh failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            r#""^[[3~" delete-char"#
        );
    }
}
