#![allow(non_upper_case_globals, non_snake_case)]

use bitflags::bitflags;
use core::ffi::c_void;
use core_foundation::{
    base::{CFTypeRef, OSStatus},
    dictionary::CFDictionaryRef,
    string::CFStringRef,
    url::CFURLRef,
};
use security_framework_sys::base::{SecKeychainItemRef, SecKeychainRef};
use std::os::raw::c_char;

#[derive(Debug)]
#[repr(C)]
pub struct SecKeychainCallbackInfo {
    pub version: u32,
    pub item: SecKeychainItemRef,
    pub keychain: SecKeychainRef,
    pub pid: i32,
}

// TODO: Put these in `security-framework-sys`?
type SecKeychainCallback = extern "C" fn(
    keychainEvent: SecKeychainEvent,
    info: *mut SecKeychainCallbackInfo,
    ctx: *mut c_void,
) -> OSStatus;

#[link(name = "Security", kind = "framework")]
extern "C" {
    pub fn SecKeychainAddCallback(
        callbackFunction: SecKeychainCallback,
        eventMask: SecKeychainEventMask,
        userContext: *mut c_void,
    ) -> OSStatus;

    // pub fn SecKeychainRemoveCallback(callbackFuncton: SecKeychainCallback) -> OSStatus;

    pub fn SecItemCopyMatching(query: CFDictionaryRef, result: *mut CFTypeRef) -> OSStatus;

    pub static kSecMatchItemList: CFStringRef;

    pub static kSecMatchLimitOne: CFStringRef;

    pub static kSecAttrModificationDate: CFStringRef;
}

bitflags! {
    #[repr(transparent)]
    pub struct SecKeychainEvent: u32 {
        const kSecLockEvent = 1;
        const kSecUnlockEvent = 2;
        const kSecAddEvent = 3;
        const kSecDeleteEvent = 4;
        const kSecUpdateEvent = 5;
        const kSecPasswordChangedEvent = 6;
        const kSecDefaultChangedEvent = 9;
        #[deprecated(note = "Read events are no longer posted macos(10.10, 10.15))")]
        const kSecDataAccessEvent = 10;
        const kSecKeychainListChangedEvent  = 11;
        const kSecTrustSettingsChangedEvent = 12;
    }
}

bitflags::bitflags! {
    #[repr(transparent)]
    pub struct SecKeychainEventMask: u32 {
        const kSecUnlockEventMask = 1 << SecKeychainEvent::kSecUnlockEvent.bits();
        const kSecAddEventMask = 1 << SecKeychainEvent::kSecAddEvent.bits();
        const kSecDeleteEventMask = 1 << SecKeychainEvent::kSecDeleteEvent.bits();
        const kSecUpdateEventMask = 1 << SecKeychainEvent::kSecUpdateEvent.bits();
        const kSecPasswordChangedEventMask = 1 << SecKeychainEvent::kSecPasswordChangedEvent.bits();
        const kSecDefaultChangedEventMask = 1 << SecKeychainEvent::kSecDefaultChangedEvent.bits();
        #[allow(deprecated)]
        #[deprecated(note = "Read events are no longer posted macos(10.10, 10.15)")]
        const kSecDataAccessEventMask = 1 << SecKeychainEvent::kSecDataAccessEvent.bits();
        const kSecKeychainListChangedMask  = 1 << SecKeychainEvent::kSecKeychainListChangedEvent.bits();
        const kSecTrustSettingsChangedEventMask = 1 << SecKeychainEvent::kSecTrustSettingsChangedEvent.bits();
        const kSecEveryEventMask = 0xffffffff;
    }
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub fn CFCopyHomeDirectoryURL() -> CFURLRef;
}

extern "C" {
    pub fn sandbox_init_with_parameters(
        profile: *const c_char,
        flags: u64,
        parameters: *const *const c_char,
        errorBuf: *mut *mut c_char,
    ) -> i32;
}

// Linker magic for the Objective-C runtime
#[link(name = "AppKit", kind = "framework")]
extern "C" {}
