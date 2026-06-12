//! PTY session management.
//!
//! Wraps [`portable_pty`] to spawn a shell and provide non-blocking
//! read/write access via a background thread.

use std::io::{BufReader, Read, Write};
use std::sync::mpsc;
use std::thread;

use portable_pty::{CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};

use zenmux_core::{Error, Result, TermSize};

/// A running PTY session connected to a shell process.
pub struct PtySession {
    /// Writer — send keyboard input to the shell (obtained via `take_writer`).
    writer: Option<Box<dyn Write + Send>>,
    /// Receiver — bytes emitted by the shell arrive here.
    rx: mpsc::Receiver<Vec<u8>>,
    /// Handle to the background reader thread.
    _reader_thread: thread::JoinHandle<()>,
    /// The master PTY handle (kept alive for resize).
    master: Box<dyn MasterPty>,
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
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Error::Pty(e.to_string()))?;

        let cmd = CommandBuilder::new_default_prog();
        let _child = pair
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
                let mut buf = [0u8; 4096];
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
            _reader_thread,
            master,
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

    /// Write bytes to the shell's stdin.
    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        if let Some(writer) = self.writer.as_mut() {
            writer.write_all(data).map_err(Error::Io)?;
            writer.flush().map_err(Error::Io)?;
            Ok(())
        } else {
            Err(Error::Pty("writer already taken".into()))
        }
    }

    /// Resize the PTY (called on window resize).
    pub fn resize(&mut self, size: TermSize) -> Result<()> {
        self.master
            .resize(PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Error::Pty(e.to_string()))
    }
}
