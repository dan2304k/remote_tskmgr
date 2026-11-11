//! Windows, `gui` feature: the binary uses the GUI subsystem (no console window pops up at
//! login), so stderr is invisible unless redirected. This module attaches to the parent's
//! console for the flags that print something and for usage errors, and shows a message box
//! for errors nobody would otherwise see.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::System::Console::{
    AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND,
};

const PRINTING_FLAGS: &[&str] = &[
    "--once",
    "--print-config",
    "--help",
    "-h",
    "--version",
    "-V",
];

static ATTACHED: AtomicBool = AtomicBool::new(false);

/// Attach to the parent process's console (once). False without one (started from Explorer,
/// the Run key, a shortcut).
fn attach_parent() -> bool {
    if ATTACHED.load(Ordering::SeqCst) {
        return true;
    }
    // SAFETY: plain Win32 call with a constant argument.
    let ok = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } != 0;
    if ok {
        ATTACHED.store(true, Ordering::SeqCst);
    }
    ok
}

/// Does stderr go anywhere (a redirect, a pipe, or an attached console)?
fn stderr_visible() -> bool {
    // SAFETY: GetStdHandle has no preconditions; the handle is only compared, not used.
    let h = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    !h.is_null() && h != INVALID_HANDLE_VALUE
}

fn message_box(title: &str, text: &str) {
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (title, text) = (wide(title), wide(text));
    // SAFETY: both pointers are NUL-terminated UTF-16 buffers that outlive the call; a null
    // owner window is allowed.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

/// Call first thing in `main`. Without a parent console (started from Explorer) this is a no-op.
pub fn attach_if_printing() {
    let printing = std::env::args_os()
        .skip(1)
        .any(|a| a.to_str().is_some_and(|s| PRINTING_FLAGS.contains(&s)));
    if printing {
        attach_parent();
    }
}

/// Command-line parsing failed: clap prints the message to stderr next, so make stderr the
/// parent console; without one, show the message in a box.
pub fn report_usage_error(e: &clap::Error) {
    if stderr_visible() || attach_parent() || !e.use_stderr() {
        return;
    }
    message_box("rtm-sender: invalid command line", &e.to_string());
}

/// A fatal error in GUI mode: it is already in the log; show it unless stderr is visible.
pub fn report_fatal(e: &anyhow::Error, log: &Path) {
    if stderr_visible() {
        return;
    }
    message_box(
        "rtm-sender",
        &format!(
            "rtm-sender stopped:\n\n{e:#}\n\nLog file: {}",
            std::path::absolute(log)
                .unwrap_or_else(|_| log.to_path_buf())
                .display()
        ),
    );
}
