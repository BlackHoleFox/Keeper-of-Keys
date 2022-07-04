use std::{ffi::CStr, os::unix::prelude::OsStrExt, path::Path, ptr};

pub fn init_sandbox(home: &Path, service_name: &'static str) {
    log::debug!("wrapping sandbox...");

    static PROFILE: &str = concat!(include_str!("../resources/sandbox.sb"), "\0");

    let mut home_dir = home.as_os_str().as_bytes().to_vec();
    home_dir.push(0);

    let exe_location = std::env::current_exe().unwrap();
    let exe_parent = exe_location.parent().unwrap().parent().unwrap();

    // In a real build, it needs to be able to read anything from inside the bundle
    // but in development its convienent to support using `cargo run` while not
    // granting excess sandbox read permissions that would make a behavior difference
    // compared to when running out of the bundle.
    let bundle_dir = if cfg!(debug_assertions) && exe_parent.ends_with("target") {
        exe_parent
    } else {
        exe_parent.parent().unwrap()
    };

    log::debug!("app location: {}", bundle_dir.display());

    let mut bundle_dir = bundle_dir.to_string_lossy().as_bytes().to_vec();
    bundle_dir.push(0);

    let mut service_name = service_name.as_bytes().to_vec();
    service_name.push(0);

    let params = [
        b"BUNDLE_PATH\0".as_ptr().cast(),
        bundle_dir.as_ptr().cast(),
        b"USER_HOME\0".as_ptr().cast(),
        home_dir.as_ptr().cast(),
        b"PING_SERVICE_NAME\0".as_ptr().cast(),
        service_name.as_ptr().cast(),
        ptr::null(),
    ];

    let mut err = ptr::null_mut();

    let status = unsafe {
        crate::bindings::sandbox_init_with_parameters(
            PROFILE.as_ptr().cast(),
            0,
            params.as_ptr(),
            &mut err,
        )
    };

    if status != 0 {
        // sandboxd logs to stderr by default when init fails, so don't duplicate messages.
        let is_terminal = atty::is(atty::Stream::Stderr);

        let _msg;
        let msg: &dyn std::fmt::Display = if !err.is_null() && !is_terminal {
            _msg = unsafe { CStr::from_ptr(err).to_string_lossy() };
            &_msg
        } else {
            &status
        };

        if !is_terminal {
            log::error!("failed to init sandbox: {}", msg);
        }

        panic!("failed to init sandbox: {msg}")
    }

    log::debug!("sandbox applied");
}
