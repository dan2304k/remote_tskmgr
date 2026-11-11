//! Windows, `gui` feature: the binary uses the GUI subsystem (no console window pops up at
//! login), so attach to the parent's console for the flags that print something.

use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};

const PRINTING_FLAGS: &[&str] = &[
    "--once",
    "--print-config",
    "--help",
    "-h",
    "--version",
    "-V",
];

/// Call first thing in `main`. Without a parent console (started from Explorer) this is a no-op.
pub fn attach_if_printing() {
    let printing = std::env::args_os()
        .skip(1)
        .any(|a| a.to_str().is_some_and(|s| PRINTING_FLAGS.contains(&s)));
    if printing {
        // SAFETY: plain Win32 call with a constant argument; failure (no parent console) is
        // harmless and ignored.
        unsafe {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}
