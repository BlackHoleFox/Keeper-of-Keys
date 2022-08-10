#[cfg(not(target_os = "macos"))]
compile_error!("only available on macOS");

use bytemuck::{Pod, Zeroable};
use const_format::formatcp;
use core_foundation::{
    base::TCFType,
    runloop::CFRunLoop,
    url::{CFURLRef, CFURL},
};
use objc::{msg_send, runtime::Class, sel, sel_impl};
use objc_foundation::{INSString, NSString};
use objc_id::Id;
use std::{ffi::OsStr, fs, path::Path, ptr::NonNull, thread, time::Duration};

mod bindings;
mod config;
use config::Config;
mod events;
use events::{EventData, EventDetails, FilteredEventData};

mod messaging;
use messaging::{Sender, Server};

mod sandbox;
mod version;

const BUNDLE_ID: &str = "org.blackholefox.keeperofkeys";
const SERVICE_NAME: &str = formatcp!("{BUNDLE_ID}.pinger");

#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq, Eq)]
#[repr(transparent)]
struct ClientRequest(u8);

impl ClientRequest {
    #![allow(non_upper_case_globals, non_snake_case)]

    const VersionInfo: Self = Self(0);
    const Shutdown: Self = Self(1);
}

#[derive(Clone, Copy, Debug, Pod, Zeroable)]
#[repr(C)]
struct ClientRequestShutdown;

#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq, PartialOrd)]
#[repr(C)]
struct AppVersion {
    major: u16,
    minor: u16,
    patch: u16,
}

#[derive(Clone, Copy, Debug, Pod, Zeroable)]
#[repr(C)]
struct ShuttingDown;

embed_plist::embed_launchd_plist!("../target/launchd.plist");

fn main() -> Result<(), ()> {
    let logger = flexi_logger::Logger::try_with_env_or_str("info").unwrap();

    let home = unsafe {
        CFURL::wrap_under_create_rule(bindings::CFCopyHomeDirectoryURL())
            .to_path()
            .unwrap()
    };

    let data_home = home.join(formatcp!("Library/Containers/{BUNDLE_ID}/Data"));

    let file_log_spec = {
        let log_dir = data_home.join("Logs");

        flexi_logger::FileSpec::default()
            .directory(log_dir)
            .basename("keeper_of_keys")
    };

    let logger = logger
        .duplicate_to_stderr(flexi_logger::Duplicate::All)
        .log_to_file(file_log_spec)
        .rotate(
            flexi_logger::Criterion::Age(flexi_logger::Age::Day),
            flexi_logger::Naming::Numbers,
            flexi_logger::Cleanup::KeepLogFiles(10),
        );

    // Not sandboxed. Writing to this works since handles opened before self-wrapping in the sandbox are still valid.
    let _logger_handle = logger.start().expect("failed to start logger");

    log::info!("initializing");

    let mut args = std::env::args().skip(1);

    match args.next() {
        Some(arg) if arg == "monitor" => sandbox::init_sandbox(&home, &data_home, SERVICE_NAME),
        _ => return register_service(&home),
    };

    let config = Config::read_from_dir(&data_home);

    Config::setup_home_link(&data_home, &home);

    // LEAK NOTE: 1 (16 bytes) ROOT LEAK: <NSArray 0x600002a80360> [16]
    mac_notification_sys::set_application(BUNDLE_ID).unwrap();

    let event_source = events::start_keychain_monitor();

    thread::Builder::new()
        .name(String::from("Status Listener"))
        .spawn(move || {
            let mut listener =
                Server::<AppVersion, ClientRequest, _, _>::create(SERVICE_NAME, |msg| match msg {
                    ClientRequest::VersionInfo => {
                        messaging::ReplyWith::new(Some(AppVersion::CURRENT), None)
                    }
                    ClientRequest::Shutdown => messaging::ReplyWith::new(
                        Some(ShuttingDown),
                        Some(|| {
                            let runloop = CFRunLoop::get_current();
                            runloop.stop();
                        }),
                    ),
                    _ => {
                        log::warn!("received bogus request kind");
                        messaging::ReplyWith::new(None::<()>, None)
                    }
                });

            // blocks forever until this thread's runloop is killed during a shutdown command.
            listener.recv_messages();

            log::info!("status listener closed, we're being replaced");
            std::process::exit(0);
        })
        .expect("failed to start status listener");

    let nsapp_class = objc::class!(NSRunningApplication);

    log::info!("setup done, waiting for events...");

    while let Ok(original_event) = event_source.recv() {
        log::trace!("raw keychain event: {:?}", original_event);

        // On at least newer versions of macOS, keychain item "editing", both through the APIs directly and Keychain Access.app, are
        // impplemented via delete -> add event sequences for every supported item type. To send a sensible notification saying something
        // was "changed", the two events need squashed into a single `Update` event.
        //
        // The events come in nearly exactly right after eachother and have identical edit timestamps.
        let ev = if let Ok(next_event) = event_source.recv_timeout(Duration::from_millis(100)) {
            log::trace!("received next event");

            match original_event {
                // If the first and second events came from the same process at the same time...
                EventData::RemovedOrUpdate {
                    seen_at,
                    modified_by,
                } if seen_at == original_event.changed_at()
                    && modified_by == original_event.changer_pid() =>
                {
                    match next_event {
                        // ... an updated occured
                        EventData::AddOrUpdate(EventDetails { details, .. }) => {
                            log::debug!("skipped duplicate event");
                            FilteredEventData::Updated(details)
                        }
                        // ... the item was actually deleted, though this branch should be unreachable with "normal" apps.
                        EventData::RemovedOrUpdate { .. } => original_event.assume_filtered(),
                    }
                }
                // If the changer and timestamp weren't identical, can't assume this was a related item and it needs processed on its own
                // during both removals or additions.
                EventData::RemovedOrUpdate { .. } => original_event.assume_filtered(),
                EventData::AddOrUpdate(_) => original_event.assume_filtered(),
            }
        } else {
            original_event.assume_filtered()
        };

        let item_title = ev.item_title().unwrap_or("Unknown");

        if config
            .ignored_items
            .iter()
            .any(|ignored| ignored == item_title)
        {
            log::debug!("skipping change, it had an ignored item title");
            continue;
        }

        let subtitle = format!("Item: {}", item_title);
        log::debug!("sending notification about {}", subtitle);

        let mut builder = mac_notification_sys::Notification::new();

        builder.title(match &ev {
            FilteredEventData::Added { .. } => "A new keychain item was added",
            FilteredEventData::Updated { .. } => "A keychain item was updated",
            FilteredEventData::Removed { .. } => "A keychain item was removed",
        });

        let message = get_changer_message(nsapp_class, ev.changer_pid());
        builder.message(&message);

        builder.subtitle(&subtitle);

        builder.send().unwrap();
    }

    log::info!("event stream closed, shutting down");
    Ok(())
}

fn register_service(home: &Path) -> Result<(), ()> {
    let launchd_plist = embed_plist::get_launchd_plist();

    let agent_dir = home.join("Library/LaunchAgents");

    fs::create_dir_all(&agent_dir).unwrap();

    let agent_path = agent_dir.join(format!("{BUNDLE_ID}.plist"));

    match Sender::connect(SERVICE_NAME) {
        Some(mut sender) => {
            let running_version: AppVersion = sender.send(ClientRequest::VersionInfo);

            log::debug!("running instance version: {running_version:?}");

            if running_version >= AppVersion::CURRENT {
                log::info!("another instance is already running, bye");
                return Ok(());
            } else {
                log::info!("replacing existing instance for update...");
                let _: ShuttingDown = sender.send(ClientRequest::Shutdown);

                run_launchctl_command("remove", BUNDLE_ID)?;

                thread::sleep(Duration::from_millis(500));
            }
        }
        None => {
            log::debug!("no other instance running, assuming service role")
        }
    }

    log::info!("registering LaunchAgent...");

    fs::write(&agent_path, launchd_plist).unwrap();

    run_launchctl_command("load", &agent_path)?;

    log::info!("LaunchAgent registered, service started, and done");

    Ok(())
}

fn run_launchctl_command(command: &str, arg: impl AsRef<OsStr>) -> Result<(), ()> {
    match std::process::Command::new("launchctl")
        .arg(command)
        .arg(arg)
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .output()
    {
        // launchctl doesn't seem to return actual status codes.
        Ok(output) => {
            if output.stderr.is_empty() {
                Ok(())
            } else {
                log::error!(
                    "failed to register agent with launchd: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                Err(())
            }
        }
        Err(e) => {
            log::error!("failed to spawn launchctl: {e}");

            Err(())
        }
    }
}

fn get_changer_message(nsapp_class: &Class, modifier: i32) -> String {
    const BASE_MSG: &str = "Changer:";

    let running_app: Option<NonNull<objc::runtime::Object>> = unsafe {
        msg_send![
            nsapp_class,
            runningApplicationWithProcessIdentifier: modifier
        ]
    };

    match running_app {
        Some(app) => {
            let app_name: Option<NonNull<NSString>> =
                unsafe { msg_send![app.as_ptr(), localizedName] };
            let app_name = app_name.map(|obj| unsafe { Id::<NSString>::from_ptr(obj.as_ptr()) });

            let exe_path: Option<NonNull<CFURLRef>> =
                unsafe { msg_send![app.as_ptr(), executableURL] };
            let exe_path = exe_path.map(|obj| unsafe {
                CFURL::wrap_under_get_rule(obj.as_ptr().cast())
                    .to_path()
                    .unwrap()
            });

            let changer = if let Some(app_name) = &app_name {
                app_name.as_str()
            } else if let Some(exe_path) = &exe_path {
                exe_path.file_name().unwrap().to_str().unwrap()
            } else {
                "Unknown Application"
            };

            format!("{BASE_MSG} {changer}")
        }
        // TODO: maybe grab the executable path instead
        None => format!("{BASE_MSG} Private Application ({modifier})"),
    }
}
