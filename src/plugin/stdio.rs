//! Taking the channel out of reach of everything else in the process.
//!
//! The app talks to a plugin over its stdin and stdout. Left where they are,
//! any `println!` (the plugin's, or a dependency's) would write into the middle
//! of a message, and a program the plugin starts with inherited stdio (`git`,
//! `claude`) would read the app's messages as its own input. So before the
//! plugin's code runs, the two are moved to private handles that children do
//! not inherit, stdin becomes empty, and stdout becomes stderr, the app's log.

use std::fs::File;

/// Move stdin and stdout aside and return them as `(from_app, to_app)`.
#[cfg(unix)]
pub fn take() -> std::io::Result<(File, File)> {
    use std::os::fd::{AsFd, AsRawFd};

    // `try_clone_to_owned` duplicates with F_DUPFD_CLOEXEC above fd 2, so the
    // copies survive the redirect below and no child inherits them.
    let from_app = std::io::stdin().as_fd().try_clone_to_owned()?;
    let to_app = std::io::stdout().as_fd().try_clone_to_owned()?;

    let null = File::open("/dev/null")?;
    // SAFETY: dup2 on descriptors this process owns; it only rebinds 0 and 1.
    unsafe {
        if libc::dup2(null.as_raw_fd(), 0) < 0 || libc::dup2(2, 1) < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok((File::from(from_app), File::from(to_app)))
}

/// Move stdin and stdout aside and return them as `(from_app, to_app)`.
#[cfg(windows)]
pub fn take() -> std::io::Result<(File, File)> {
    use std::os::windows::io::{FromRawHandle, IntoRawHandle};
    use windows_sys::Win32::Foundation::{
        HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
    };

    fn check(h: HANDLE) -> std::io::Result<HANDLE> {
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(h)
        }
    }

    // SAFETY: Win32 calls on this process's own standard handles. The two pipe
    // handles change hands exactly once, into the returned `File`s.
    unsafe {
        let from_app = check(GetStdHandle(STD_INPUT_HANDLE))?;
        let to_app = check(GetStdHandle(STD_OUTPUT_HANDLE))?;
        let log = check(GetStdHandle(STD_ERROR_HANDLE))?;
        // Children start with bInheritHandles, so the pipes must not be
        // inheritable or a child would hold the app's channel open.
        SetHandleInformation(from_app, HANDLE_FLAG_INHERIT, 0);
        SetHandleInformation(to_app, HANDLE_FLAG_INHERIT, 0);
        // Rust's stdout looks its handle up on every write, so `println!`
        // follows this to stderr.
        let null = File::open("NUL")?.into_raw_handle();
        if SetStdHandle(STD_INPUT_HANDLE, null as HANDLE) == 0
            || SetStdHandle(STD_OUTPUT_HANDLE, log) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok((
            File::from_raw_handle(from_app as _),
            File::from_raw_handle(to_app as _),
        ))
    }
}
