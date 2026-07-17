//! Worker threads. Replaces the C app's fire-and-forget detached pthreads
//! (which mutated shared state with no locks): workers here receive plain
//! data, send plain results over an mpsc channel, and poke a wake pipe so
//! the idle main loop notices. All tree mutation happens on the main thread,
//! gated by generational-id checks so results for closed nodes are dropped.

use std::io;
use std::os::fd::RawFd;
use std::path::PathBuf;
use std::sync::mpsc;

use crate::model::{self, ItemData, NodeId};

pub enum TaskResult {
    ScanDone {
        node: NodeId,
        item: usize,
        path: PathBuf,
        result: io::Result<Vec<ItemData>>,
    },
    PreviewDone {
        gen: u64,
        image: Option<crate::preview::DecodedImage>,
    },
}

pub struct Tasks {
    pub rx: mpsc::Receiver<TaskResult>,
    tx: mpsc::Sender<TaskResult>,
    wake_write: RawFd,
    pub wake_read: RawFd,
}

impl Tasks {
    pub fn new() -> Result<Tasks, String> {
        let mut fds = [0i32; 2];
        let ret = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) };
        if ret != 0 {
            return Err(format!("pipe2: {}", io::Error::last_os_error()));
        }
        let (tx, rx) = mpsc::channel();
        Ok(Tasks { rx, tx, wake_write: fds[1], wake_read: fds[0] })
    }

    /// Drain the wake pipe after poll() saw it readable.
    pub fn drain_wake(&self) {
        let mut buf = [0u8; 64];
        loop {
            let n = unsafe { libc::read(self.wake_read, buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                break;
            }
        }
    }

    pub fn wake_fd(fd: RawFd) {
        unsafe { libc::write(fd, [1u8].as_ptr().cast(), 1) };
    }

    /// Sender + wake fd for long-lived workers (the preview thread).
    pub fn result_sender(&self) -> (mpsc::Sender<TaskResult>, RawFd) {
        (self.tx.clone(), self.wake_write)
    }

    /// Scan `path` on an ephemeral worker thread; the result is applied on
    /// the main thread only if (node, item) still resolves and is still
    /// marked scanning.
    pub fn spawn_scan(&self, node: NodeId, item: usize, path: PathBuf) {
        let tx = self.tx.clone();
        let wake_fd = self.wake_write;
        std::thread::spawn(move || {
            // Debug aid for exercising close-during-scan and spinner states.
            if let Some(ms) =
                std::env::var("KEXPLORE_SCAN_DELAY_MS").ok().and_then(|v| v.parse::<u64>().ok())
            {
                std::thread::sleep(std::time::Duration::from_millis(ms));
            }
            let result = model::scan_dir(&path);
            if tx.send(TaskResult::ScanDone { node, item, path, result }).is_ok() {
                Self::wake_fd(wake_fd);
            }
        });
    }
}
