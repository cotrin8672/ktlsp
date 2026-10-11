#![allow(missing_docs)]
//! Watcher implementation for Windows' directory management APIs
//!
//! For more information see the [ReadDirectoryChangesW reference][ref].
//!
//! [ref]: https://msdn.microsoft.com/en-us/library/windows/desktop/aa363950(v=vs.85).aspx

use crate::{bounded, unbounded, BoundSender, Config, Receiver, Sender};
use crate::{event::*, WatcherKind};
use crate::{Error, EventHandler, RecursiveMode, Result, Watcher};
use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
use std::io;
use std::os::raw::c_void;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_NOTIFY_ENUM_DIR, ERROR_SUCCESS, HANDLE,
    INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_IO_COMPLETION, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FileStandardInfo, GetFileInformationByHandleEx, ReadDirectoryChangesW,
    FILE_ACTION_ADDED, FILE_ACTION_MODIFIED,
    FILE_ACTION_REMOVED, FILE_ACTION_RENAMED_NEW_NAME, FILE_ACTION_RENAMED_OLD_NAME,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_DIR_NAME,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SECURITY,
    FILE_NOTIFY_CHANGE_SIZE, FILE_NOTIFY_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_STANDARD_INFO, OPEN_EXISTING,
};
use windows_sys::Win32::System::Threading::{
    CreateSemaphoreW, ReleaseSemaphore, SleepEx, WaitForSingleObjectEx, INFINITE,
};
use windows_sys::Win32::System::IO::{CancelIo, OVERLAPPED};

const BUF_SIZE: u32 = 16384;

#[derive(Default)]
struct ReadState {
    stopping: AtomicBool,
    pending: AtomicBool,
}

#[derive(Clone)]
struct ReadData {
    watch_path: PathBuf,   // key used in the server watch map
    dir: PathBuf,          // directory that is being watched
    file: Option<PathBuf>, // if a file is being watched, this is its full path
    is_recursive: bool,
    state: Arc<ReadState>,
}

// The native notification buffer must be DWORD-aligned, including on 32-bit Windows.
#[repr(C)]
struct ReadDirectoryRequest {
    event_handler: Arc<Mutex<dyn EventHandler>>,
    buffer: [u8; BUF_SIZE as usize],
    handle: HANDLE,
    data: ReadData,
    action_tx: Sender<Action>,
}

impl ReadDirectoryRequest {
    fn unwatch(&self) {
        let _ = self.action_tx.send(Action::Unwatch(
            self.data.watch_path.clone(),
            self.data.state.clone(),
        ));
    }
}

enum Action {
    Watch(PathBuf, RecursiveMode),
    Unwatch(PathBuf, Arc<ReadState>),
    UnwatchAck(PathBuf),
    Stop,
    Configure(Config, BoundSender<Result<bool>>),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MetaEvent {
    SingleWatchComplete,
    WatcherAwakened,
}

struct WatchState {
    dir_handle: HANDLE,
    state: Arc<ReadState>,
}

struct ReadDirectoryChangesServer {
    tx: Sender<Action>,
    rx: Receiver<Action>,
    event_handler: Arc<Mutex<dyn EventHandler>>,
    meta_tx: Sender<MetaEvent>,
    cmd_tx: Sender<Result<PathBuf>>,
    watches: HashMap<PathBuf, WatchState>,
    wakeup: Arc<OwnedHandle>,
}

impl ReadDirectoryChangesServer {
    fn start(
        event_handler: Arc<Mutex<dyn EventHandler>>,
        meta_tx: Sender<MetaEvent>,
        cmd_tx: Sender<Result<PathBuf>>,
        wakeup: Arc<OwnedHandle>,
        builder: thread::Builder,
    ) -> Result<(Sender<Action>, thread::JoinHandle<()>)> {
        let (action_tx, action_rx) = unbounded();
        let worker = builder
            .spawn({
                let tx = action_tx.clone();
                move || {
                    let server = ReadDirectoryChangesServer {
                        tx,
                        rx: action_rx,
                        event_handler,
                        meta_tx,
                        cmd_tx,
                        watches: HashMap::new(),
                        wakeup,
                    };
                    server.run();
                }
            })
            .map_err(Error::io)?;
        Ok((action_tx, worker))
    }

    fn run(mut self) {
        loop {
            // process all available actions first
            let mut stopped = false;

            while let Ok(action) = self.rx.try_recv() {
                match action {
                    Action::Watch(path, recursive_mode) => {
                        let res = self.add_watch(path, recursive_mode.is_recursive());
                        let _ = self.cmd_tx.send(res);
                    }
                    Action::Unwatch(path, state) => {
                        self.remove_watch_if_current(path, &state);
                    }
                    Action::UnwatchAck(path) => {
                        self.remove_watch(path.clone());
                        let _ = self.cmd_tx.send(Ok(path));
                    }
                    Action::Stop => {
                        stopped = true;
                        break;
                    }
                    Action::Configure(config, tx) => {
                        self.configure_raw_mode(config, tx);
                    }
                }
            }

            if stopped {
                break;
            }

            unsafe {
                // wait with alertable flag so that the completion routine fires
                match WaitForSingleObjectEx(self.wakeup.as_raw_handle(), 100, 1) {
                    WAIT_OBJECT_0 => {
                        let _ = self.meta_tx.send(MetaEvent::WatcherAwakened);
                    }
                    WAIT_IO_COMPLETION | WAIT_TIMEOUT => {}
                    result => {
                        let error = if result == WAIT_FAILED {
                            Error::io(io::Error::last_os_error())
                        } else {
                            Error::generic(&format!("Unexpected watch wait result: {result}"))
                        };
                        emit_event(&self.event_handler, Err(error));
                        break;
                    }
                }
            }
        }

    }

    fn add_watch(&mut self, path: PathBuf, is_recursive: bool) -> Result<PathBuf> {
        // path must exist and be either a file or directory
        if !path.is_dir() && !path.is_file() {
            return Err(
                Error::generic("Input watch path is neither a file nor a directory.")
                    .add_path(path),
            );
        }

        let (watching_file, dir_target) = {
            if path.is_dir() {
                (false, path.clone())
            } else {
                // emulate file watching by watching the parent directory
                (true, path.parent().unwrap().to_path_buf())
            }
        };

        let encoded_path: Vec<u16> = dir_target
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let handle;
        unsafe {
            handle = CreateFileW(
                encoded_path.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_DELETE | FILE_SHARE_WRITE,
                ptr::null_mut(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                ptr::null_mut(),
            );

            if handle == INVALID_HANDLE_VALUE {
                return Err(Error::io_watch(io::Error::last_os_error()).add_path(path));
            }
        }
        let wf = if watching_file {
            Some(path.clone())
        } else {
            None
        };
        let state = Arc::new(ReadState::default());
        let rd = ReadData {
            watch_path: path.clone(),
            dir: dir_target,
            file: wf,
            is_recursive,
            state: state.clone(),
        };
        let ws = WatchState {
            dir_handle: handle,
            state,
        };
        if let Err(error) = start_read(&rd, self.event_handler.clone(), handle, self.tx.clone()) {
            if unsafe { CloseHandle(handle) } == 0 {
                emit_event(&self.event_handler, Err(Error::io(io::Error::last_os_error()).add_path(path)));
            }
            return Err(error);
        }
        self.remove_watch(path.clone());
        self.watches.insert(path.clone(), ws);
        Ok(path)
    }

    fn remove_watch(&mut self, path: PathBuf) {
        if let Some(ws) = self.watches.remove(&path) {
            stop_watch(&path, &ws, &self.event_handler, &self.meta_tx);
        }
    }

    fn remove_watch_if_current(&mut self, path: PathBuf, state: &Arc<ReadState>) {
        if self.watches.get(&path).is_some_and(|ws| Arc::ptr_eq(&ws.state, state)) {
            self.remove_watch(path);
        }
    }

    fn configure_raw_mode(&mut self, _config: Config, tx: BoundSender<Result<bool>>) {
        let _ = tx.send(Ok(false));
    }
}

impl Drop for ReadDirectoryChangesServer {
    fn drop(&mut self) {
        for (path, ws) in self.watches.drain() {
            stop_watch(&path, &ws, &self.event_handler, &self.meta_tx);
        }
    }
}

fn stop_watch(
    path: &Path,
    ws: &WatchState,
    event_handler: &Mutex<dyn EventHandler>,
    meta_tx: &Sender<MetaEvent>,
) {
    // Queued successful completions must not rearm a handle that is being closed.
    ws.state.stopping.store(true, Ordering::Release);
    unsafe {
        if CancelIo(ws.dir_handle) == 0 {
            emit_event(event_handler, Err(Error::io(io::Error::last_os_error()).add_path(path.to_owned())));
        }
        if CloseHandle(ws.dir_handle) == 0 {
            emit_event(event_handler, Err(Error::io(io::Error::last_os_error()).add_path(path.to_owned())));
        }
        // No semaphore failure can release a still-owned native request. Completion callbacks
        // run on this issuing thread during the alertable wait and clear `pending` themselves.
        while ws.state.pending.load(Ordering::Acquire) {
            SleepEx(INFINITE, 1);
        }
    }
    let _ = meta_tx.send(MetaEvent::SingleWatchComplete);
}

fn start_read(
    rd: &ReadData,
    event_handler: Arc<Mutex<dyn EventHandler>>,
    handle: HANDLE,
    action_tx: Sender<Action>,
) -> Result<()> {
    let request = Box::new(ReadDirectoryRequest {
        event_handler,
        handle,
        buffer: [0u8; BUF_SIZE as usize],
        data: rd.clone(),
        action_tx,
    });

    let flags = FILE_NOTIFY_CHANGE_FILE_NAME
        | FILE_NOTIFY_CHANGE_DIR_NAME
        | FILE_NOTIFY_CHANGE_ATTRIBUTES
        | FILE_NOTIFY_CHANGE_SIZE
        | FILE_NOTIFY_CHANGE_LAST_WRITE
        | FILE_NOTIFY_CHANGE_CREATION
        | FILE_NOTIFY_CHANGE_SECURITY;

    let monitor_subdir = if request.data.file.is_none() && request.data.is_recursive {
        1
    } else {
        0
    };

    unsafe {
        let mut overlapped = Box::new(std::mem::zeroed::<OVERLAPPED>());
        // When using callback based async requests, we are allowed to use the hEvent member
        // for our own purposes

        let request = Box::into_raw(request);
        overlapped.hEvent = request as _;
        let overlapped = Box::into_raw(overlapped);
        rd.state.pending.store(true, Ordering::Release);

        // This is using an asynchronous call with a completion routine for receiving notifications
        // An I/O completion port would probably be more performant
        let ret = ReadDirectoryChangesW(
            handle,
            (*request).buffer.as_mut_ptr() as *mut c_void,
            BUF_SIZE,
            monitor_subdir,
            flags,
            &mut 0u32 as *mut u32, // not used for async reqs
            overlapped,
            Some(handle_event),
        );

        if ret == 0 {
            // error reading. retransmute request memory to allow drop.
            // Because of the error, ownership of the `overlapped` alloc was not passed
            // over to `ReadDirectoryChangesW`.
            // So we can claim ownership back.
            let error = io::Error::last_os_error();
            let _overlapped = Box::from_raw(overlapped);
            let request = Box::from_raw(request);
            request.data.state.pending.store(false, Ordering::Release);
            return Err(Error::io(error).add_path(request.data.watch_path.clone()));
        }
    }
    Ok(())
}

fn emit_event(event_handler: &Mutex<dyn EventHandler>, result: Result<Event>) {
    let mut handler = event_handler.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    handler.handle_event(result);
}

fn completion_rescan_event(error_code: u32, bytes_written: u32) -> Option<Event> {
    (error_code == ERROR_NOTIFY_ENUM_DIR || (error_code == ERROR_SUCCESS && bytes_written == 0))
        .then(|| Event::new(EventKind::Other).set_flag(Flag::Rescan))
}

fn is_delete_pending(handle: HANDLE) -> bool {
    let mut info = unsafe { std::mem::zeroed::<FILE_STANDARD_INFO>() };
    let result = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileStandardInfo,
            &mut info as *mut _ as *mut c_void,
            std::mem::size_of_val(&info) as u32,
        )
    };
    result != 0 && info.DeletePending
}

fn notification_record(buffer: &[u8]) -> Option<(u32, OsString, usize)> {
    let header_len = std::mem::offset_of!(FILE_NOTIFY_INFORMATION, FileName);
    let header = buffer.get(..header_len)?;
    let next = u32::from_le_bytes(header.get(..4)?.try_into().ok()?) as usize;
    let action = u32::from_le_bytes(header.get(4..8)?.try_into().ok()?);
    let length = u32::from_le_bytes(header.get(8..12)?.try_into().ok()?) as usize;
    let end = header_len.checked_add(length)?;
    if length == 0 || length % 2 != 0 || (next != 0 && (next % 2 != 0 || next < end || next >= buffer.len())) {
        return None;
    }
    let filename = buffer.get(header_len..end)?;
    // Wine can use two-byte rather than four-byte record alignment.
    if filename.as_ptr().align_offset(std::mem::align_of::<u16>()) != 0 {
        return None;
    }
    let wide = unsafe { slice::from_raw_parts(filename.as_ptr().cast::<u16>(), length / 2) };
    Some((action, OsString::from_wide(wide), next))
}

unsafe extern "system" fn handle_event(
    error_code: u32,
    bytes_written: u32,
    overlapped: *mut OVERLAPPED,
) {
    let overlapped: Box<OVERLAPPED> = Box::from_raw(overlapped);
    let request: Box<ReadDirectoryRequest> = Box::from_raw(overlapped.hEvent as *mut _);
    request.data.state.pending.store(false, Ordering::Release);
    if request.data.state.stopping.load(Ordering::Acquire) {
        return;
    }

    if error_code != ERROR_SUCCESS && error_code != ERROR_NOTIFY_ENUM_DIR {
        if error_code == ERROR_ACCESS_DENIED && request.data.file.is_none()
            && (matches!(request.data.dir.try_exists(), Ok(false)) || is_delete_pending(request.handle))
        {
            emit_event(&request.event_handler, Ok(Event::new(EventKind::Remove(RemoveKind::Folder)).add_path(request.data.dir.clone())));
        }
        // Cancellation is expected only after `stopping` is set; every other terminal code
        // must reach the caller, including access revoked while the path still exists.
        emit_event(&request.event_handler, Err(Error::io(io::Error::from_raw_os_error(error_code as i32)).add_path(request.data.watch_path.clone())));
        request.unwatch();
        return;
    }

    let rearm_error = start_read(
        &request.data,
        request.event_handler.clone(),
        request.handle,
        request.action_tx.clone(),
    ).err();

    if let Some(event) = completion_rescan_event(error_code, bytes_written) {
        emit_event(&request.event_handler, Ok(event));
    } else if let Some(mut remaining) = request.buffer.get(..bytes_written as usize) {
        loop {
            let Some((action, filename, next)) = notification_record(remaining) else {
                emit_event(&request.event_handler, Ok(Event::new(EventKind::Other).set_flag(Flag::Rescan)));
                emit_event(&request.event_handler, Err(Error::generic("Malformed directory notification").add_path(request.data.watch_path.clone())));
                break;
            };
            let path = request.data.dir.join(filename);
            if request.data.file.as_ref().map_or(true, |file| *file == path) {
                let kind = match action {
                    FILE_ACTION_RENAMED_OLD_NAME => Some(EventKind::Modify(ModifyKind::Name(RenameMode::From))),
                    FILE_ACTION_RENAMED_NEW_NAME => Some(EventKind::Modify(ModifyKind::Name(RenameMode::To))),
                    FILE_ACTION_ADDED => Some(EventKind::Create(CreateKind::Any)),
                    FILE_ACTION_REMOVED => Some(EventKind::Remove(RemoveKind::Any)),
                    FILE_ACTION_MODIFIED => Some(EventKind::Modify(ModifyKind::Any)),
                    _ => None,
                };
                if let Some(kind) = kind {
                    emit_event(&request.event_handler, Ok(Event::new(kind).add_path(path)));
                } else {
                    emit_event(&request.event_handler, Ok(Event::new(EventKind::Other).set_flag(Flag::Rescan)));
                    emit_event(&request.event_handler, Err(Error::generic("Unknown directory notification action").add_path(path)));
                }
            }
            if next == 0 {
                break;
            }
            remaining = &remaining[next..];
        }
    } else {
        emit_event(&request.event_handler, Ok(Event::new(EventKind::Other).set_flag(Flag::Rescan)));
        emit_event(&request.event_handler, Err(Error::generic("Invalid directory notification byte count").add_path(request.data.watch_path.clone())));
    }

    // A rescan and failed rearm are separate conditions; do not hide the terminal error.
    if let Some(error) = rearm_error {
        emit_event(&request.event_handler, Err(error));
        request.unwatch();
    }
}

/// Watcher implementation based on ReadDirectoryChanges
#[derive(Debug)]
pub struct ReadDirectoryChangesWatcher {
    tx: Sender<Action>,
    cmd_rx: Receiver<Result<PathBuf>>,
    wakeup: Arc<OwnedHandle>,
    worker: Option<thread::JoinHandle<()>>,
}

impl ReadDirectoryChangesWatcher {
    pub fn create(
        event_handler: Arc<Mutex<dyn EventHandler>>,
        meta_tx: Sender<MetaEvent>,
    ) -> Result<ReadDirectoryChangesWatcher> {
        let (cmd_tx, cmd_rx) = unbounded();

        let wakeup_sem = unsafe { CreateSemaphoreW(ptr::null_mut(), 0, 1, ptr::null_mut()) };
        if wakeup_sem.is_null() || wakeup_sem == INVALID_HANDLE_VALUE {
            return Err(Error::io(io::Error::last_os_error()));
        }

        let wakeup = Arc::new(unsafe { OwnedHandle::from_raw_handle(wakeup_sem) });
        let (action_tx, worker) = ReadDirectoryChangesServer::start(
            event_handler,
            meta_tx,
            cmd_tx,
            wakeup.clone(),
            thread::Builder::new().name("notify-rs windows loop".to_string()),
        )?;

        Ok(ReadDirectoryChangesWatcher {
            tx: action_tx,
            cmd_rx,
            wakeup,
            worker: Some(worker),
        })
    }

    fn wakeup_server(&mut self) {
        // breaks the server out of its wait state.  right now this is really just an optimization,
        // so that if you add a watch you don't block for 100ms in watch() while the
        // server sleeps.
        unsafe {
            ReleaseSemaphore(self.wakeup.as_raw_handle(), 1, ptr::null_mut());
        }
    }

    fn send_action_require_ack(&mut self, action: Action, pb: &PathBuf) -> Result<()> {
        self.ensure_not_callback()?;
        self.tx
            .send(action)
            .map_err(|_| Error::generic("Error sending to internal channel"))?;

        // wake 'em up, we don't want to wait around for the ack
        self.wakeup_server();

        let ack_pb = self
            .cmd_rx
            .recv()
            .map_err(|_| Error::generic("Error receiving from command channel"))??;

        if pb.as_path() != ack_pb.as_path() {
            Err(Error::generic(&format!(
                "Expected ack for {:?} but got \
                 ack for {:?}",
                pb, ack_pb
            )))
        } else {
            Ok(())
        }
    }

    fn watch_inner(&mut self, path: &Path, recursive_mode: RecursiveMode) -> Result<()> {
        let pb = if path.is_absolute() {
            path.to_owned()
        } else {
            let p = env::current_dir().map_err(Error::io)?;
            p.join(path)
        };
        // path must exist and be either a file or directory
        if !pb.is_dir() && !pb.is_file() {
            return Err(Error::generic(
                "Input watch path is neither a file nor a directory.",
            ));
        }
        self.send_action_require_ack(Action::Watch(pb.clone(), recursive_mode), &pb)
    }

    fn unwatch_inner(&mut self, path: &Path) -> Result<()> {
        let pb = if path.is_absolute() {
            path.to_owned()
        } else {
            let p = env::current_dir().map_err(Error::io)?;
            p.join(path)
        };
        self.send_action_require_ack(Action::UnwatchAck(pb.clone()), &pb)
    }

    fn ensure_not_callback(&self) -> Result<()> {
        if self.worker.as_ref().is_some_and(|worker| worker.thread().id() == thread::current().id()) {
            Err(Error::generic("Blocking watcher methods cannot run in the event callback"))
        } else {
            Ok(())
        }
    }
}

// The receiver is only used by `&mut self` methods; preserve the existing public Sync contract.
unsafe impl Sync for ReadDirectoryChangesWatcher {}
impl Watcher for ReadDirectoryChangesWatcher {
    fn new<F: EventHandler>(event_handler: F, _config: Config) -> Result<Self> {
        // create dummy channel for meta event
        // TODO: determine the original purpose of this - can we remove it?
        let (meta_tx, _) = unbounded();
        let event_handler = Arc::new(Mutex::new(event_handler));
        Self::create(event_handler, meta_tx)
    }

    fn watch(&mut self, path: &Path, recursive_mode: RecursiveMode) -> Result<()> {
        self.watch_inner(path, recursive_mode)
    }

    fn unwatch(&mut self, path: &Path) -> Result<()> {
        self.unwatch_inner(path)
    }

    fn configure(&mut self, config: Config) -> Result<bool> {
        self.ensure_not_callback()?;
        let (tx, rx) = bounded(1);
        self.tx.send(Action::Configure(config, tx))?;
        self.wakeup_server();
        rx.recv()?
    }

    fn kind() -> crate::WatcherKind {
        WatcherKind::ReadDirectoryChangesWatcher
    }
}

impl Drop for ReadDirectoryChangesWatcher {
    fn drop(&mut self) {
        let _ = self.tx.send(Action::Stop);
        // better wake it up
        self.wakeup_server();
        if let Some(worker) = self.worker.take() {
            // Self-drop cannot synchronously join its own callback. The server still owns the
            // wakeup handle and drains its watches after this callback returns.
            if worker.thread().id() != thread::current().id() {
                if worker.join().is_err() {
                    log::error!("notify-rs Windows loop panicked during shutdown");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;
    use windows_sys::Win32::Foundation::{ERROR_INVALID_HANDLE, ERROR_OPERATION_ABORTED};

    fn data(state: Arc<ReadState>) -> ReadData {
        ReadData {
            watch_path: PathBuf::from(r"C:\watched\file.txt"),
            dir: PathBuf::from(r"C:\watched"),
            file: Some(PathBuf::from(r"C:\watched\file.txt")),
            is_recursive: false,
            state,
        }
    }

    fn deliver(code: u32, bytes: &[u8], data: ReadData) -> (Vec<Result<Event>>, Vec<Action>) {
        let (event_tx, event_rx) = mpsc::channel::<Result<Event>>();
        let (action_tx, action_rx) = unbounded();
        let mut request = Box::new(ReadDirectoryRequest {
            event_handler: Arc::new(Mutex::new(event_tx)),
            buffer: [0u8; BUF_SIZE as usize],
            handle: INVALID_HANDLE_VALUE,
            data,
            action_tx,
        });
        request.buffer[..bytes.len()].copy_from_slice(bytes);
        let mut overlapped = Box::new(unsafe { std::mem::zeroed::<OVERLAPPED>() });
        overlapped.hEvent = Box::into_raw(request) as _;
        unsafe { handle_event(code, bytes.len() as u32, Box::into_raw(overlapped)) };
        (event_rx.try_iter().collect(), action_rx.try_iter().collect())
    }

    fn record(action: u32, name: &str, next: u32) -> Vec<u8> {
        let wide = name.encode_utf16().collect::<Vec<_>>();
        let mut bytes = next.to_le_bytes().to_vec();
        bytes.extend(action.to_le_bytes());
        bytes.extend(((wide.len() * 2) as u32).to_le_bytes());
        for code in wide {
            bytes.extend(code.to_le_bytes());
        }
        bytes
    }

    fn wakeup() -> Arc<OwnedHandle> {
        let handle = unsafe { CreateSemaphoreW(ptr::null_mut(), 0, 1, ptr::null_mut()) };
        assert!(!handle.is_null());
        assert_ne!(handle, INVALID_HANDLE_VALUE);
        Arc::new(unsafe { OwnedHandle::from_raw_handle(handle) })
    }

    #[test]
    fn lost_details_produce_pathless_rescan() {
        for code in [ERROR_SUCCESS, ERROR_NOTIFY_ENUM_DIR] {
            let event = completion_rescan_event(code, 0).expect("rescan event");
            assert_eq!(event.kind, EventKind::Other);
            assert!(event.paths.is_empty());
            assert!(event.need_rescan());
        }
        assert!(completion_rescan_event(ERROR_SUCCESS, 12).is_none());
        assert!(completion_rescan_event(ERROR_OPERATION_ABORTED, 0).is_none());
    }

    #[test]
    fn failed_initial_arm_returns_io_error_and_reclaims_request() {
        let state = Arc::new(ReadState::default());
        let rd = data(state.clone());
        let (event_tx, event_rx) = mpsc::channel::<Result<Event>>();
        let (action_tx, action_rx) = unbounded();
        let error = start_read(&rd, Arc::new(Mutex::new(event_tx)), INVALID_HANDLE_VALUE, action_tx).expect_err("invalid handle cannot arm");
        assert!(matches!(error.kind, crate::ErrorKind::Io(_)));
        assert_eq!(error.paths, [rd.watch_path]);
        assert!(!state.pending.load(Ordering::Acquire));
        assert!(event_rx.try_iter().next().is_none());
        assert!(action_rx.try_iter().next().is_none());
    }

    #[test]
    fn stopped_successful_completion_does_not_rearm() {
        let state = Arc::new(ReadState::default());
        state.stopping.store(true, Ordering::Release);
        state.pending.store(true, Ordering::Release);
        let (events, actions) = deliver(ERROR_SUCCESS, &[], data(state.clone()));
        assert!(events.is_empty());
        assert!(actions.is_empty());
        assert!(!state.pending.load(Ordering::Acquire));
    }

    #[test]
    fn every_terminal_completion_reports_error_and_exact_watch_owner() {
        for code in [ERROR_INVALID_HANDLE, ERROR_ACCESS_DENIED, ERROR_OPERATION_ABORTED, 59] {
            let state = Arc::new(ReadState::default());
            let rd = data(state.clone());
            let (events, actions) = deliver(code, &[], rd.clone());
            assert!(events.iter().any(|event| matches!(event,
                Err(Error { kind: crate::ErrorKind::Io(error), paths })
                if error.raw_os_error() == Some(code as i32) && paths == &[rd.watch_path.clone()]
            )));
            assert!(matches!(&actions[..], [Action::Unwatch(path, owner)]
                if path == &rd.watch_path && Arc::ptr_eq(owner, &state)));
            assert!(!state.pending.load(Ordering::Acquire));
        }
        let mut rd = data(Arc::new(ReadState::default()));
        rd.file = None;
        rd.dir = PathBuf::from(OsString::from_wide(&[0]));
        assert!(rd.dir.try_exists().is_err());
        let (events, _) = deliver(ERROR_ACCESS_DENIED, &[], rd);
        assert_eq!(events.len(), 1);
        assert!(events.iter().all(Result::is_err), "an existence error is not proof of deletion");
    }

    #[test]
    fn overflow_and_failed_rearm_are_both_delivered() {
        for code in [ERROR_SUCCESS, ERROR_NOTIFY_ENUM_DIR] {
            let state = Arc::new(ReadState::default());
            let (events, actions) = deliver(code, &[], data(state.clone()));
            assert_eq!(events.len(), 2);
            assert!(events[0].as_ref().expect("rescan").need_rescan());
            assert!(events[1].is_err());
            assert!(matches!(&actions[..], [Action::Unwatch(_, owner)] if Arc::ptr_eq(owner, &state)));
        }
    }

    #[test]
    fn bounded_record_parser_accepts_wine_alignment_and_rejects_bad_lengths() {
        let first = record(FILE_ACTION_ADDED, "a", 14);
        let second = record(FILE_ACTION_MODIFIED, "b", 0);
        let bytes = [first.clone(), second.clone()].concat();
        let (action, filename, next) = notification_record(&bytes).expect("first record");
        assert_eq!((action, filename, next), (FILE_ACTION_ADDED, OsString::from("a"), 14));
        assert_eq!(notification_record(&bytes[next..]).expect("last record").1, OsString::from("b"));
        assert!(notification_record(&bytes[..11]).is_none());
        assert!(notification_record(&second[..13]).is_none());
        let mut odd = second.clone();
        odd[8..12].copy_from_slice(&1u32.to_le_bytes());
        assert!(notification_record(&odd).is_none());
        let mut overlap = bytes.clone();
        overlap[..4].copy_from_slice(&12u32.to_le_bytes());
        assert!(notification_record(&overlap).is_none());
        let mut past_end = bytes;
        past_end[..4].copy_from_slice(&100u32.to_le_bytes());
        assert!(notification_record(&past_end).is_none());
    }

    #[test]
    fn rename_from_to_and_single_file_filter_are_preserved() {
        let mut rd = data(Arc::new(ReadState::default()));
        rd.file = None;
        let first = record(FILE_ACTION_RENAMED_OLD_NAME, "old.txt", 26);
        let second = record(FILE_ACTION_RENAMED_NEW_NAME, "new.txt", 0);
        let bytes = [first, second].concat();
        let (events, _) = deliver(ERROR_SUCCESS, &bytes, rd.clone());
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].as_ref().unwrap(), &Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::From))).add_path(rd.dir.join("old.txt")));
        assert_eq!(events[1].as_ref().unwrap(), &Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::To))).add_path(rd.dir.join("new.txt")));
        assert!(events[2].is_err(), "failed rearm must still be reported");
        rd.file = Some(rd.dir.join("old.txt"));
        let (events, _) = deliver(ERROR_SUCCESS, &bytes, rd);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].as_ref().unwrap().kind, EventKind::Modify(ModifyKind::Name(RenameMode::From)));
        assert!(events[1].is_err());
    }

    #[test]
    fn malformed_native_record_requests_rescan_without_out_of_bounds_read() {
        let (events, _) = deliver(ERROR_SUCCESS, &[0, 1, 2], data(Arc::new(ReadState::default())));
        assert!(events[0].as_ref().unwrap().need_rescan());
        assert!(events[1].is_err());
        assert!(events[2].is_err(), "failed rearm is a second failure");
    }

    #[test]
    fn thread_spawn_failure_releases_wakeup_owner() {
        let wakeup = wakeup();
        let weak = Arc::downgrade(&wakeup);
        let (event_tx, _event_rx) = mpsc::channel::<Result<Event>>();
        let (meta_tx, _meta_rx) = unbounded();
        let (cmd_tx, _cmd_rx) = unbounded();
        let result = ReadDirectoryChangesServer::start(
            Arc::new(Mutex::new(event_tx)), meta_tx, cmd_tx, wakeup,
            thread::Builder::new().stack_size(usize::MAX),
        );
        let failed = result.is_err();
        if let Ok((tx, worker)) = result {
            let _ = tx.send(Action::Stop);
            if let Some(wakeup) = weak.upgrade() {
                unsafe { ReleaseSemaphore(wakeup.as_raw_handle(), 1, ptr::null_mut()) };
            }
            worker.join().unwrap();
        }
        assert!(failed, "invalid stack reservation must fail creation");
        assert!(weak.upgrade().is_none(), "failed spawn retained the native handle");
    }

    #[test]
    fn old_self_unwatch_cannot_remove_replacement() {
        let dir = tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let (event_tx, _event_rx) = mpsc::channel::<Result<Event>>();
        let (tx, rx) = unbounded();
        let (meta_tx, meta_rx) = unbounded();
        let (cmd_tx, _cmd_rx) = unbounded();
        let mut server = ReadDirectoryChangesServer {
            tx, rx, event_handler: Arc::new(Mutex::new(event_tx)), meta_tx, cmd_tx,
            watches: HashMap::new(), wakeup: wakeup(),
        };
        server.add_watch(path.clone(), false).unwrap();
        let old = server.watches[&path].state.clone();
        server.add_watch(path.clone(), false).unwrap();
        let current = server.watches[&path].state.clone();
        server.remove_watch_if_current(path.clone(), &old);
        let retained = server.watches.get(&path).is_some_and(|watch| Arc::ptr_eq(&watch.state, &current));
        server.remove_watch_if_current(path, &current);
        assert!(retained);
        assert!(server.watches.is_empty());
        assert!(!old.pending.load(Ordering::Acquire));
        assert!(!current.pending.load(Ordering::Acquire));
        assert_eq!(meta_rx.try_iter().filter(|event| *event == MetaEvent::SingleWatchComplete).count(), 2);
    }

    #[test]
    fn unwatch_and_drop_wait_for_pending_callback() {
        use std::sync::atomic::AtomicUsize;

        for use_unwatch in [true, false] {
            let dir = tempdir().unwrap();
            let watched = dir.path().join("old");
            let control_path = dir.path().join("control");
            std::fs::create_dir(&watched).unwrap();
            std::fs::create_dir(&control_path).unwrap();
            let (control_tx, control_rx) = mpsc::channel::<Result<Event>>();
            let mut control = ReadDirectoryChangesWatcher::new(control_tx, Config::default()).unwrap();
            control.watch(&control_path, RecursiveMode::NonRecursive).unwrap();

            let (started_tx, started_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let (done_tx, done_rx) = mpsc::channel::<Option<ReadDirectoryChangesWatcher>>();
            let blocked = Arc::new(AtomicBool::new(false));
            let callbacks = Arc::new(AtomicUsize::new(0));
            let errors = Arc::new(AtomicUsize::new(0));
            let barrier_expired = Arc::new(AtomicBool::new(false));
            let callback_count = callbacks.clone();
            let callback_errors = errors.clone();
            let callback_expired = barrier_expired.clone();
            let mut watcher = ReadDirectoryChangesWatcher::new(move |event: Result<Event>| {
                callback_count.fetch_add(1, Ordering::AcqRel);
                if event.is_err() {
                    callback_errors.fetch_add(1, Ordering::AcqRel);
                }
                if event.is_ok() && !blocked.swap(true, Ordering::AcqRel) {
                    let _ = started_tx.send(());
                    // A failed test must not strand its own deliberately blocked callback.
                    if release_rx.recv_timeout(Duration::from_secs(10)).is_err() {
                        callback_expired.store(true, Ordering::Release);
                    }
                }
            }, Config::default()).unwrap();
            watcher.watch(&watched, RecursiveMode::NonRecursive).unwrap();
            std::fs::write(watched.join("first.txt"), b"first").unwrap();
            started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let worker_path = watched.clone();
            let join = thread::spawn(move || {
                if use_unwatch {
                    watcher.unwatch(&worker_path).unwrap();
                    // Keep the unregistered watcher alive throughout the post-ACK writes:
                    // Drop must not accidentally supply the guarantee attributed to unwatch.
                    let _ = done_tx.send(Some(watcher));
                } else {
                    drop(watcher);
                    let _ = done_tx.send(None);
                }
            });
            let early = done_rx.recv_timeout(Duration::from_millis(100));
            let returned_early = early.is_ok();
            let _ = release_tx.send(());
            let unregistered = match early {
                Ok(value) => value,
                Err(_) => done_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            };
            join.join().unwrap();
            let at_ack = callbacks.load(Ordering::Acquire);
            let old_file = watched.join("after-ack.txt");
            let renamed_file = watched.join("after-rename.txt");
            for operation in 0..3 {
                match operation {
                    0 => std::fs::write(&old_file, b"after unwatch or drop").unwrap(),
                    1 => std::fs::rename(&old_file, &renamed_file).unwrap(),
                    _ => std::fs::remove_file(&renamed_file).unwrap(),
                }
                // Prove that actual native event delivery is still functioning after each
                // old-path operation; do not use a quiet sleep as the sole barrier.
                let marker = control_path.join(format!("barrier-{operation}.txt"));
                std::fs::write(&marker, b"control").unwrap();
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut delivered = false;
                while !delivered && Instant::now() < deadline {
                    if let Ok(event) = control_rx.recv_timeout(Duration::from_millis(100)) {
                        delivered = event.expect("independent native control watch failed").paths.contains(&marker);
                    }
                }
                assert!(delivered, "independent native control event missing, operation={operation}");
                assert_eq!(callbacks.load(Ordering::Acquire), at_ack,
                    "old callback after ACK/drop, operation={operation}");
            }
            drop(unregistered);
            drop(control);
            eprintln!("NATIVE_CALLBACK_STOP mode={} ack_counter={} final_counter={} errors={}",
                if use_unwatch { "unwatch-live" } else { "drop-joined" }, at_ack,
                callbacks.load(Ordering::Acquire), errors.load(Ordering::Acquire));
            assert!(!returned_early, "shutdown acknowledged before callback drained");
            assert!(!barrier_expired.load(Ordering::Acquire), "test callback barrier expired");
            assert_eq!(errors.load(Ordering::Acquire), 0, "unexpected old-watch callback error");
            assert_eq!(callbacks.load(Ordering::Acquire), at_ack, "callback during final retired drop");
        }
    }

    #[test]
    fn directory_removal_reaches_client_as_remove_and_terminal_error() {
        let dir = tempdir().unwrap();
        let watched = dir.path().join("watched");
        std::fs::create_dir(&watched).unwrap();
        let (tx, rx) = mpsc::channel::<Result<Event>>();
        let mut watcher = ReadDirectoryChangesWatcher::new(tx, Config::default()).unwrap();
        watcher.watch(&watched, RecursiveMode::NonRecursive).unwrap();
        std::fs::remove_dir(&watched).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut removed, mut failed) = (false, false);
        while !(removed && failed) && Instant::now() < deadline {
            if let Ok(result) = rx.recv_timeout(Duration::from_millis(100)) {
                match result {
                    Ok(event) => removed |= event.kind == EventKind::Remove(RemoveKind::Folder) && event.paths == [watched.clone()],
                    Err(_) => failed = true,
                }
            }
        }
        drop(watcher);
        assert!(removed, "watched-directory removal was lost");
        assert!(failed, "terminated watch was not reported");
    }

    #[test]
    fn nonrecursive_parent_observes_unicode_root_rename_and_fresh_root_delete_error() {
        let dir = tempdir().unwrap();
        let old_root = dir.path().join("監視-旧-🧪");
        let new_root = dir.path().join("監視-新-🌲");
        std::fs::create_dir(&old_root).unwrap();
        let (parent_tx, parent_rx) = mpsc::channel::<Result<Event>>();
        let mut parent = ReadDirectoryChangesWatcher::new(parent_tx, Config::default()).unwrap();
        parent.watch(dir.path(), RecursiveMode::NonRecursive).unwrap();
        let (old_tx, old_rx) = mpsc::channel::<Result<Event>>();
        let mut old_watch = ReadDirectoryChangesWatcher::new(old_tx, Config::default()).unwrap();
        old_watch.watch(&old_root, RecursiveMode::NonRecursive).unwrap();
        std::fs::rename(&old_root, &new_root).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut from, mut to) = (false, false);
        while !(from && to) && Instant::now() < deadline {
            if let Ok(event) = parent_rx.recv_timeout(Duration::from_millis(100)) {
                let event = event.expect("parent watch failed during root rename");
                from |= event.kind == EventKind::Modify(ModifyKind::Name(RenameMode::From))
                    && event.paths == [old_root.clone()];
                to |= event.kind == EventKind::Modify(ModifyKind::Name(RenameMode::To))
                    && event.paths == [new_root.clone()];
            }
        }
        // A root-self watch is not required to fail on rename. Retire its old path identity,
        // then independently arm the renamed root before testing its removal contract.
        drop(old_watch);
        let old_self_events = old_rx.try_iter().collect::<Vec<_>>();
        eprintln!("NATIVE_ROOT_RENAME parent_from={from} parent_to={to} root_self_events={:?}",
            &old_self_events[..old_self_events.len().min(8)]);
        assert!(from && to, "NonRecursive parent lost exact Unicode From/To paths");

        let (root_tx, root_rx) = mpsc::channel::<Result<Event>>();
        let mut root = ReadDirectoryChangesWatcher::new(root_tx, Config::default()).unwrap();
        root.watch(&new_root, RecursiveMode::NonRecursive).unwrap();
        std::fs::remove_dir(&new_root).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut removed, mut failed) = (false, false);
        let mut native_errors = Vec::new();
        while !(removed && failed) && Instant::now() < deadline {
            if let Ok(result) = root_rx.recv_timeout(Duration::from_millis(100)) {
                match result {
                    Ok(event) => removed |= event.kind == EventKind::Remove(RemoveKind::Folder)
                        && event.paths == [new_root.clone()],
                    Err(error) => {
                        failed |= error.paths == [new_root.clone()];
                        native_errors.push(error);
                    }
                }
            }
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut parent_removed = false;
        while !parent_removed && Instant::now() < deadline {
            if let Ok(event) = parent_rx.recv_timeout(Duration::from_millis(100)) {
                let event = event.expect("parent watch failed during root deletion");
                parent_removed = matches!(event.kind, EventKind::Remove(_))
                    && event.paths == [new_root.clone()];
            }
        }
        drop(root);
        drop(parent);
        eprintln!("NATIVE_ROOT_DELETE parent_remove={parent_removed} root_remove={removed} root_error={failed} raw_errors={native_errors:?}");
        assert!(parent_removed, "parent lost renamed-root deletion");
        assert!(removed, "fresh Unicode root removal was lost");
        assert!(failed, "fresh root termination error missing or wrong owner path");
    }

    #[test]
    #[ignore = "bounded actual kernel overflow attempt; NOT_EXERCISED is not PASS"]
    fn bounded_real_kernel_overflow_while_callback_blocked_requires_rescan() {
        use std::sync::atomic::AtomicUsize;

        const WRITES: usize = 2048;
        let dir = tempdir().unwrap();
        let watched = dir.path().to_path_buf();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (sample_tx, sample_rx) = mpsc::sync_channel::<Result<Event>>(64);
        let blocked = Arc::new(AtomicBool::new(false));
        let rescans = Arc::new(AtomicUsize::new(0));
        let errors = Arc::new(AtomicUsize::new(0));
        let dropped_samples = Arc::new(AtomicUsize::new(0));
        let barrier_expired = Arc::new(AtomicBool::new(false));
        let callback_rescans = rescans.clone();
        let callback_errors = errors.clone();
        let callback_dropped = dropped_samples.clone();
        let callback_expired = barrier_expired.clone();
        let mut watcher = ReadDirectoryChangesWatcher::new(move |event: Result<Event>| {
            if event.as_ref().map_or(false, Event::need_rescan) {
                callback_rescans.fetch_add(1, Ordering::AcqRel);
            }
            if event.is_err() {
                callback_errors.fetch_add(1, Ordering::AcqRel);
            }
            if event.is_ok() && !blocked.swap(true, Ordering::AcqRel) {
                // Production handle_event rearms before calling this handler. The issuing
                // thread is now intentionally non-alertable while the finite writer runs.
                let _ = started_tx.send(());
                if release_rx.recv_timeout(Duration::from_secs(30)).is_err() {
                    callback_expired.store(true, Ordering::Release);
                }
            }
            // Bounded diagnostic samples cannot block callbacks or stand in for kernel Rescan.
            if sample_tx.try_send(event).is_err() {
                callback_dropped.fetch_add(1, Ordering::AcqRel);
            }
        }, Config::default()).unwrap();
        watcher.watch(&watched, RecursiveMode::NonRecursive).unwrap();
        std::fs::write(watched.join("seed.txt"), b"seed").unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let writer_path = watched.clone();
        let writer = thread::spawn(move || -> io::Result<usize> {
            for n in 0..WRITES {
                let name = format!("burst-{n:04}-{}.txt", "x".repeat(120));
                std::fs::write(writer_path.join(name), b"x")?;
            }
            Ok(WRITES)
        });
        let writer_result = writer.join();
        let _ = release_tx.send(());
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut samples = Vec::new();
        while rescans.load(Ordering::Acquire) == 0 && Instant::now() < deadline {
            if let Ok(event) = sample_rx.recv_timeout(Duration::from_millis(100)) {
                if samples.len() < 8 {
                    samples.push(event);
                }
            }
        }
        drop(watcher);
        let rescan_count = rescans.load(Ordering::Acquire);
        let error_count = errors.load(Ordering::Acquire);
        let expired = barrier_expired.load(Ordering::Acquire);
        let write_ok = matches!(&writer_result, Ok(Ok(WRITES)));
        let status = if !write_ok || expired || error_count != 0 {
            "FAILED"
        } else if rescan_count == 0 {
            "NOT_EXERCISED"
        } else {
            "EXERCISED"
        };
        eprintln!("NATIVE_KERNEL_OVERFLOW status={status} writes={writer_result:?} native_rescans={rescan_count} native_errors={error_count} diagnostic_samples_dropped={} barrier_expired={expired} samples={samples:?}",
            dropped_samples.load(Ordering::Acquire));
        assert!(write_ok, "finite native writer failed");
        assert!(!expired, "callback barrier expired before the finite burst finished");
        assert_eq!(error_count, 0, "native failure during overflow attempt");
        assert!(rescan_count != 0,
            "NOT_EXERCISED: real kernel did not emit Rescan; synthetic substitution is forbidden");
    }

    #[test]
    fn public_send_sync_and_native_buffer_alignment_are_preserved() {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<ReadDirectoryChangesWatcher>();
        assert_eq!(std::mem::offset_of!(ReadDirectoryRequest, buffer) % 4, 0);
    }
}
