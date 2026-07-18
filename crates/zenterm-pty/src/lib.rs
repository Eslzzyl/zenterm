//! PTY session management.
//!
//! Wraps [`portable_pty`] to spawn a shell and provide non-blocking
//! read/write access via a background thread.

use std::io::{BufReader, Read, Write};
use std::sync::mpsc;
use std::thread;

use portable_pty::{Child, CommandBuilder, ExitStatus, MasterPty, NativePtySystem, PtySize, PtySystem};

use zenterm_core::{Error, Result, TermSize};

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
    /// Uses the user's default shell.
    pub fn spawn(size: TermSize) -> Result<Self> {
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
        // Declare we are zenterm, so programs querying TERM_PROGRAM (e.g.
        // ratatui-image's Picker) don't mistake us for another terminal.
        cmd.env("TERM_PROGRAM", "zenterm");
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
        // over a channel to the main thread.
        let (tx, rx) = mpsc::channel();
        let _reader_thread = thread::Builder::new()
            .name("pty-reader".into())
            .spawn(move || {
                let mut reader = BufReader::new(reader);
                let mut buf = [0u8; 65536];
                log::debug!("pty-reader thread started");
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => {
                            // EOF — shell exited.
                            log::debug!("pty-reader: EOF from PTY");
                            let _ = tx.send(Vec::new());
                            break;
                        }
                        Ok(n) => {
                            log::debug!("pty-reader: read {} bytes from PTY: {:02x?}", n, &buf[..n]);
                            if tx.send(buf[..n].to_vec()).is_err() {
                                log::debug!("pty-reader: channel closed, exiting");
                                break;
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
                    log::debug!("pty try_read: shell exited");
                    Some(Err(Error::Pty("shell exited".into())))
                } else {
                    log::debug!("pty try_read: {} bytes: {:02x?}", data.len(), data);
                    Some(Ok(data))
                }
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                log::debug!("pty try_read: reader thread disconnected");
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
