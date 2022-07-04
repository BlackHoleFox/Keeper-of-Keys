use core::ffi::c_void;
use core_foundation::{
    array::CFArray,
    base::{OSStatus, TCFType},
    boolean::CFBoolean,
    date::CFDate,
    dictionary::{CFDictionary, CFMutableDictionary},
    runloop::{self, CFRunLoop},
    string::CFString,
};
use security_framework_sys::item::{
    kSecAttrLabel, kSecClass, kSecClassGenericPassword, kSecClassInternetPassword, kSecMatchLimit,
    kSecReturnAttributes,
};
use std::{sync::mpsc, thread, time::Duration};

use crate::bindings::{self, SecKeychainCallbackInfo, SecKeychainEvent, SecKeychainEventMask};

#[derive(Debug)]
enum AddedOrUpdated {
    Added,
    Updated,
}

#[derive(Debug)]
pub struct InnerDetails {
    item_name: String,
    modified_at: f64,
    modified_by: i32,
}

#[derive(Debug)]
pub struct EventDetails {
    pub details: InnerDetails,
    kind: AddedOrUpdated,
}

pub enum FilteredEventData {
    Added(InnerDetails),
    Updated(InnerDetails),
    Removed { seen_at: f64, modified_by: i32 },
}

impl FilteredEventData {
    pub fn changer_pid(&self) -> i32 {
        match self {
            FilteredEventData::Added(InnerDetails { modified_by, .. }) => *modified_by,
            FilteredEventData::Updated(InnerDetails { modified_by, .. }) => *modified_by,
            FilteredEventData::Removed { modified_by, .. } => *modified_by,
        }
    }

    pub fn item_title(&self) -> Option<&str> {
        match self {
            FilteredEventData::Added(InnerDetails { item_name, .. }) => Some(item_name.as_str()),
            FilteredEventData::Updated(InnerDetails { item_name, .. }) => Some(item_name.as_str()),
            FilteredEventData::Removed { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum EventData {
    AddOrUpdate(EventDetails),
    RemovedOrUpdate { seen_at: f64, modified_by: i32 },
}

impl EventData {
    pub fn changer_pid(&self) -> i32 {
        match &self {
            EventData::AddOrUpdate(EventDetails { details, .. }) => details.modified_by,
            EventData::RemovedOrUpdate { modified_by, .. } => *modified_by,
        }
    }

    pub fn changed_at(&self) -> f64 {
        match &self {
            EventData::AddOrUpdate(EventDetails { details, .. }) => details.modified_at,
            EventData::RemovedOrUpdate { seen_at, .. } => *seen_at,
        }
    }

    pub fn assume_filtered(self) -> FilteredEventData {
        match self {
            EventData::AddOrUpdate(details) => match details.kind {
                AddedOrUpdated::Added => FilteredEventData::Added(details.details),
                AddedOrUpdated::Updated => FilteredEventData::Updated(details.details),
            },
            EventData::RemovedOrUpdate {
                seen_at,
                modified_by,
            } => FilteredEventData::Removed {
                seen_at,
                modified_by,
            },
        }
    }
}

pub fn start_keychain_monitor() -> mpsc::Receiver<EventData> {
    let (tx, event_source) = mpsc::channel::<EventData>();

    thread::Builder::new()
        .name(String::from("Keychain Monitor"))
        .spawn(move || {
            let tx = Box::into_raw(Box::new(tx));

            let status = unsafe {
                bindings::SecKeychainAddCallback(
                    callback_handler,
                    SecKeychainEventMask::kSecAddEventMask
                        | SecKeychainEventMask::kSecDeleteEventMask
                        | SecKeychainEventMask::kSecUpdateEventMask,
                    tx.cast(),
                )
            };
            assert_eq!(status, 0, "failed to register callback");

            // There has got to be a better way than this D:
            // at some point, look into a custom runloop mode with a different runloop mode?
            // let loop_mode = CFString::from_static_string("org.blackholefox.KeychainMonitor");
            loop {
                CFRunLoop::run_in_mode(
                    unsafe { runloop::kCFRunLoopDefaultMode },
                    Duration::from_secs(10),
                    true,
                );

                thread::sleep(Duration::from_millis(100));

                // How does the loop end? When the system shuts down the daemon. When is the callback removed?
                // The heat death of the process.
            }
        })
        .expect("failed to start keychain monitor");

    event_source
}

#[allow(non_snake_case)]
extern "C" fn callback_handler(
    keychainEvent: SecKeychainEvent,
    info: *mut SecKeychainCallbackInfo,
    ctx: *mut c_void,
) -> OSStatus {
    let sender = unsafe { &*(ctx as *const c_void as *const mpsc::Sender<EventData>) };

    log::trace!("received callback for {:?} event", keychainEvent);

    let info = unsafe { &*info };

    if info.item.is_null() {
        log::warn!("received unusable event with no item");
        return 0;
    }

    let items = CFArray::from_copyable(&[info.item]);

    let mut query = unsafe {
        CFMutableDictionary::from_CFType_pairs(&[
            (bindings::kSecMatchItemList.cast(), items.as_CFTypeRef()),
            (
                kSecReturnAttributes.cast(),
                CFBoolean::true_value().as_CFTypeRef(),
            ),
            (kSecMatchLimit.cast(), bindings::kSecMatchLimitOne.cast()),
        ])
    };

    // TODO: support more types, like keys?
    let mut supported_item_types =
        unsafe { [kSecClassGenericPassword, kSecClassInternetPassword].into_iter() };

    let attributes = loop {
        let class = match supported_item_types.next() {
            Some(c) => c,
            None => {
                let now = CFDate::now();

                log::trace!("item was removed or not supported");

                // item wasnt there (or not supported), so consider that it may be deleted.
                // we only know for sure if there isn't another event right after it.
                //
                // branch is taken when `kSecClassInternetPassword` items are updated.
                send_event(
                    sender,
                    EventData::RemovedOrUpdate {
                        seen_at: now.abs_time().floor(),
                        modified_by: info.pid,
                    },
                );

                return 0;
            }
        };

        query.set(unsafe { kSecClass.cast() }, class.cast());

        let mut attributes = std::ptr::null();

        let status =
            unsafe { bindings::SecItemCopyMatching(query.as_concrete_TypeRef(), &mut attributes) };

        match status {
            0 => {
                let attributes: CFDictionary<CFString, *const c_void> =
                    unsafe { CFDictionary::wrap_under_create_rule(attributes.cast()) };

                break attributes;
            }
            security_framework_sys::base::errSecItemNotFound => {
                continue;
            }
            code => panic!("failed to get item attributes {code}"),
        }
    };

    let label = unsafe {
        attributes
            .find(kSecAttrLabel)
            .map(|ptr| CFString::wrap_under_get_rule(ptr.cast()))
            .unwrap()
    };

    let mtime = unsafe {
        attributes
            .find(bindings::kSecAttrModificationDate)
            .map(|ptr| CFDate::wrap_under_get_rule(ptr.cast()))
            .unwrap()
    };

    let kind = match keychainEvent {
        _ if keychainEvent.contains(SecKeychainEvent::kSecAddEvent) => AddedOrUpdated::Added,
        _ if keychainEvent.contains(SecKeychainEvent::kSecDeleteEvent) => {
            send_event(
                sender,
                EventData::RemovedOrUpdate {
                    seen_at: mtime.abs_time(),
                    modified_by: info.pid,
                },
            );
            return 0;
        }
        // below is dead code but whatever. This event never fires on modern macOS versions :(
        _ if keychainEvent.contains(SecKeychainEvent::kSecUpdateEvent) => AddedOrUpdated::Updated,
        _ => unreachable!("system returned unwanted event type"),
    };

    log::trace!("item was added or updated");

    send_event(
        sender,
        EventData::AddOrUpdate(EventDetails {
            details: InnerDetails {
                item_name: label.to_string(),
                modified_at: mtime.abs_time(),
                modified_by: info.pid,
            },
            kind,
        }),
    );

    0
}

fn send_event(sender: &mpsc::Sender<EventData>, event: EventData) {
    if sender.send(event).is_err() {
        log::warn!("event stream receiver has shutdown")
    }
}
