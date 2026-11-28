//! Check for updates on Windows. A background thread asks GitHub for the latest release over
//! WinHTTP, downloads the program and SHA256SUMS.txt, checks the sha256 and leaves the new copy
//! next to the running one. The UI thread then swaps the two and starts the new copy, which waits
//! for this one to exit. Nothing about the user is sent: only the app's name, as the User-Agent.

use crate::messages::WM_APP_UPDATE;
use crate::taskbar_dock::to_wide;
use netspeedgauge_shared::update::{asset_url, expected_sha, is_newer, is_sha_match, latest_release_url, release_tag, user_agent, windows_asset_name, UpdateStatus};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HWND, SYSTEMTIME};
use windows_sys::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable, WinHttpQueryHeaders, WinHttpReadData,
    WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts, INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
    WINHTTP_FLAG_SECURE, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
};
use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

/// The program's name in the release files and the User-Agent.
const PROGRAM_NAME: &str = "NetSpeedGauge";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const SUMS_NAME: &str = "SHA256SUMS.txt";
const RELEASE_ACCEPT: &str = "application/vnd.github+json";
const FILE_ACCEPT: &str = "application/octet-stream";
/// The new copy starts with this and the old copy's process id, and waits for it to exit.
const AFTER_UPDATE_ARGUMENT: &str = "--after-update=";
const OLD_COPY_WAIT_MS: u32 = 15_000;
const TIMEOUT_MS: i32 = 15_000;
const HTTP_OK: u32 = 200;
// Far above either program's size, so a broken answer cannot fill the disk.
const MAX_DOWNLOAD_BYTES: usize = 64 * 1024 * 1024;
// Seconds from 1601, where Windows' clock starts, to 1970.
const WINDOWS_EPOCH_OFFSET: u64 = 11_644_473_600;
const TICKS_PER_SECOND: u64 = 10_000_000;
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// What a finished check hands the UI thread.
pub struct Outcome {
    /// When GitHub answered, to save as the last check; None when it could not be reached.
    pub checked_at: Option<u64>,
    /// The checked new copy waiting next to the running program.
    pub new_copy: Option<PathBuf>,
}

static STATUS: Mutex<UpdateStatus> = Mutex::new(UpdateStatus::Idle);
static OUTCOME: Mutex<Option<Outcome>> = Mutex::new(None);
static IS_BUSY: AtomicBool = AtomicBool::new(false);

/// Returns where the Check now row stands.
pub fn status() -> UpdateStatus {
    STATUS.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Sets the row's state and tells the main window, which redraws Settings.
fn set_status(window: isize, status: UpdateStatus) {
    *STATUS.lock().unwrap_or_else(PoisonError::into_inner) = status;
    // SAFETY: posting to the app's main window; a closed window only drops the message.
    unsafe { PostMessageW(window as HWND, WM_APP_UPDATE, 0, 0) };
}

/// Takes the last finished check, once.
pub fn take_outcome() -> Option<Outcome> {
    OUTCOME.lock().unwrap_or_else(PoisonError::into_inner).take()
}

/// Returns the time now in seconds since 1970.
pub fn now_seconds() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_secs())
}

/// Starts a check on a background thread, unless one is running. Each step arrives as WM_APP_UPDATE.
pub fn check_now(window: HWND) {
    if IS_BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    let window = window as isize;
    std::thread::spawn(move || {
        set_status(window, UpdateStatus::Checking);
        let (status, outcome) = run_check(window);
        *OUTCOME.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
        IS_BUSY.store(false, Ordering::SeqCst);
        set_status(window, status);
    });
}

/// Asks GitHub for the latest release and, when it is newer, downloads and checks it.
fn run_check(window: isize) -> (UpdateStatus, Outcome) {
    let agent = user_agent(PROGRAM_NAME);
    let unreachable = (UpdateStatus::Failed, Outcome { checked_at: None, new_copy: None });
    let Some(json) = latest_release_url(REPOSITORY).and_then(|url| fetch(&url, &agent, RELEASE_ACCEPT)).and_then(|body| String::from_utf8(body).ok()) else {
        return unreachable;
    };
    let checked = Outcome { checked_at: Some(now_seconds()), new_copy: None };
    let Some(tag) = release_tag(&json) else { return (UpdateStatus::Failed, checked) };
    if !is_newer(VERSION, &tag) {
        return (UpdateStatus::UpToDate, checked);
    }
    set_status(window, UpdateStatus::Downloading(tag.clone()));
    match download_checked(&json, &agent).and_then(|program| write_new_copy(&program)) {
        Some(new_copy) => (UpdateStatus::Ready(tag), Outcome { new_copy: Some(new_copy), ..checked }),
        None => (UpdateStatus::Failed, checked),
    }
}

/// Downloads this machine's program and keeps it only when its sha256 matches SHA256SUMS.txt and it is a Windows program.
fn download_checked(json: &str, agent: &str) -> Option<Vec<u8>> {
    let asset = windows_asset_name(PROGRAM_NAME, cfg!(target_arch = "aarch64"));
    let sums = fetch(&asset_url(json, SUMS_NAME)?, agent, FILE_ACCEPT).and_then(|body| String::from_utf8(body).ok())?;
    let expected = expected_sha(&sums, &asset)?;
    let program = fetch(&asset_url(json, &asset)?, agent, FILE_ACCEPT)?;
    // Every Windows program starts with "MZ"; anything else is not one, whatever its name.
    (is_sha_match(&program, &expected) && program.starts_with(b"MZ")).then_some(program)
}

/// Returns "NetSpeedGauge.exe.<ending>" next to the running program.
fn sibling(program: &Path, ending: &str) -> PathBuf {
    let name = program.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    program.with_file_name(format!("{name}.{ending}"))
}

fn write_new_copy(program: &[u8]) -> Option<PathBuf> {
    let new_copy = sibling(&std::env::current_exe().ok()?, "new");
    std::fs::write(&new_copy, program).ok()?;
    Some(new_copy)
}

/**
 * Swaps the running program for the new copy and starts it; the caller quits right after. Windows
 * lets a running program be renamed, so the old copy moves aside and the new one takes its exact
 * path: Start with Windows stays as it is. Returns false, with
 * the running copy back in place, when any step fails.
 */
pub fn install(new_copy: &Path) -> bool {
    let Ok(program) = std::env::current_exe() else { return false };
    let old_copy = sibling(&program, "old");
    let _ = std::fs::remove_file(&old_copy);
    if std::fs::rename(&program, &old_copy).is_err() {
        let _ = std::fs::remove_file(new_copy);
        return false;
    }
    if std::fs::rename(new_copy, &program).is_err() {
        let _ = std::fs::rename(&old_copy, &program);
        return false;
    }
    if Command::new(&program).arg(format!("{AFTER_UPDATE_ARGUMENT}{}", std::process::id())).spawn().is_err() {
        let _ = std::fs::rename(&program, new_copy);
        let _ = std::fs::rename(&old_copy, &program);
        return false;
    }
    true
}

/// Runs first at launch: after an update it waits for the old copy to exit, then removes the leftovers.
pub fn finish_update() {
    let old_process = std::env::args().find_map(|argument| argument.strip_prefix(AFTER_UPDATE_ARGUMENT).and_then(|id| id.parse::<u32>().ok()));
    if let Some(process_id) = old_process {
        // SAFETY: a wait-only handle on the old copy, closed right after.
        unsafe {
            let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, process_id);
            if !process.is_null() {
                WaitForSingleObject(process, OLD_COPY_WAIT_MS);
                CloseHandle(process);
            }
        }
    }
    if let Ok(program) = std::env::current_exe() {
        let _ = std::fs::remove_file(sibling(&program, "old"));
        let _ = std::fs::remove_file(sibling(&program, "new"));
    }
}

/// Returns a time in seconds since 1970 as local "28 Nov 15:20", or None before the first check.
pub fn time_label(seconds: u64) -> Option<String> {
    if seconds == 0 {
        return None;
    }
    let ticks = (seconds + WINDOWS_EPOCH_OFFSET) * TICKS_PER_SECOND;
    let file_time = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    // SAFETY: plain conversions into zeroed structs owned here.
    unsafe {
        let (mut utc, mut local): (SYSTEMTIME, SYSTEMTIME) = (std::mem::zeroed(), std::mem::zeroed());
        if FileTimeToSystemTime(&file_time, &mut utc) == 0 || SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) == 0 {
            return None;
        }
        let month = MONTHS[usize::from(local.wMonth.clamp(1, 12)) - 1];
        Some(format!("{} {month} {:02}:{:02}", local.wDay, local.wHour, local.wMinute))
    }
}

/// Closes a WinHTTP handle when it goes out of scope.
struct Handle(*mut core::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: a handle WinHTTP gave us, closed once.
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

/// GETs an https address and returns the body of a 200 answer. WinHTTP follows GitHub's redirect to its file store.
fn fetch(url: &str, agent: &str, accept: &str) -> Option<Vec<u8>> {
    let rest = url.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/').map_or((rest, "/".to_string()), |(host, path)| (host, format!("/{path}")));
    let (agent, host, path, verb) = (to_wide(agent), to_wide(host), to_wide(&path), to_wide("GET"));
    let headers = to_wide(&format!("Accept: {accept}\r\n"));
    // SAFETY: every string is null-terminated and outlives the calls; each handle closes on drop, children first.
    unsafe {
        let session = Handle(WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, std::ptr::null(), std::ptr::null(), 0));
        if session.0.is_null() {
            return None;
        }
        WinHttpSetTimeouts(session.0, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS);
        let connection = Handle(WinHttpConnect(session.0, host.as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0));
        if connection.0.is_null() {
            return None;
        }
        let request = Handle(WinHttpOpenRequest(connection.0, verb.as_ptr(), path.as_ptr(), std::ptr::null(), std::ptr::null(), std::ptr::null(), WINHTTP_FLAG_SECURE));
        if request.0.is_null() || WinHttpSendRequest(request.0, headers.as_ptr(), u32::MAX, std::ptr::null(), 0, 0, 0) == 0 || WinHttpReceiveResponse(request.0, std::ptr::null_mut()) == 0 {
            return None;
        }
        let mut code = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let flags = WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER;
        if WinHttpQueryHeaders(request.0, flags, std::ptr::null(), std::ptr::from_mut(&mut code).cast(), &mut size, std::ptr::null_mut()) == 0 || code != HTTP_OK {
            return None;
        }
        let mut body = Vec::new();
        loop {
            let mut available = 0u32;
            if WinHttpQueryDataAvailable(request.0, &mut available) == 0 {
                return None;
            }
            if available == 0 {
                return Some(body);
            }
            let start = body.len();
            if start + available as usize > MAX_DOWNLOAD_BYTES {
                return None;
            }
            body.resize(start + available as usize, 0);
            let mut read = 0u32;
            if WinHttpReadData(request.0, body[start..].as_mut_ptr().cast(), available, &mut read) == 0 {
                return None;
            }
            body.truncate(start + read as usize);
        }
    }
}
