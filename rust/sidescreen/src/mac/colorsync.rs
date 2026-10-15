//! ColorSync's records of Sidescreen's displays.
//!
//! macOS keeps a ColorSync record for every display serial it has seen, and
//! each time displays change WindowServer asks colorsync.displayservices
//! for every online display's profile. The service reads all records about
//! four times per online display. WindowServer waits 50 ms for the answer;
//! a later one is dropped, and it asks again 5 s later, until an answer
//! comes in time. Every display change starts another such loop. With many
//! records no answer comes in time, the loops pile up, and WindowServer
//! and ColorSync stay busy for good.
//!
//! Sidescreen reuses serials (`display::Serial`), so it adds a record only
//! per display it holds at once. Versions before that left one per display
//! ever made; `Sidescreen colorsync clean` removes those, as root, since
//! ColorSync asks for an administrator to remove a record.

use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::string::{CFString, CFStringRef};
use core_foundation::uuid::{CFUUID, CFUUIDRef};
use std::collections::BTreeMap;
use std::ffi::c_void;

#[link(name = "ColorSync", kind = "framework")]
unsafe extern "C" {
    fn ColorSyncIterateDeviceProfiles(
        callback: extern "C" fn(CFDictionaryRef, *mut c_void) -> bool,
        user_info: *mut c_void,
    );
    fn ColorSyncUnregisterDevice(device_class: CFStringRef, device_id: CFUUIDRef) -> bool;
    fn CGDisplayCreateUUIDFromDisplayID(display: u32) -> CFUUIDRef;
    static kColorSyncDisplayDeviceClass: CFStringRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGGetOnlineDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
}

unsafe extern "C" {
    fn CFUUIDCreateString(allocator: *const c_void, uuid: CFUUIDRef) -> CFStringRef;
}

fn text_of(id: &CFUUID) -> String {
    unsafe {
        CFString::wrap_under_create_rule(CFUUIDCreateString(
            std::ptr::null(),
            id.as_concrete_TypeRef(),
        ))
    }
    .to_string()
}

/// A display ColorSync has a record of.
pub struct Record {
    pub id: CFUUID,
    pub name: String,
    pub online: bool,
}

impl Record {
    /// One of Sidescreen's displays, not connected now.
    pub fn is_stale(&self) -> bool {
        !self.online && self.name.starts_with("Sidescreen")
    }
}

extern "C" fn collect(info: CFDictionaryRef, all: *mut c_void) -> bool {
    let all = unsafe { &mut *(all as *mut BTreeMap<String, (CFUUID, String)>) };
    let info: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_get_rule(info) };
    let text = |key: &str| {
        info.find(CFString::new(key))
            .and_then(|value| value.downcast::<CFString>())
            .map(|value| value.to_string())
    };
    if text("DeviceClass").as_deref() != Some("mntr") {
        return true;
    }
    let Some(id) = info
        .find(CFString::new("DeviceID"))
        .and_then(|value| value.downcast::<CFUUID>())
    else {
        return true;
    };
    let key = text_of(&id);
    all.entry(key)
        .or_insert_with(|| (id, text("DeviceDescription").unwrap_or_default()));
    true
}

/// Every display ColorSync has a record of. Takes up to a second with many.
pub fn records() -> Vec<Record> {
    let mut all: BTreeMap<String, (CFUUID, String)> = BTreeMap::new();
    unsafe { ColorSyncIterateDeviceProfiles(collect, &mut all as *mut _ as *mut c_void) };
    let mut ids = [0u32; 32];
    let mut count = 0u32;
    unsafe { CGGetOnlineDisplayList(32, ids.as_mut_ptr(), &mut count) };
    let online: Vec<String> = ids[..count as usize]
        .iter()
        .filter_map(|&display| {
            let id = unsafe { CGDisplayCreateUUIDFromDisplayID(display) };
            (!id.is_null()).then(|| text_of(&unsafe { CFUUID::wrap_under_create_rule(id) }))
        })
        .collect();
    all.into_iter()
        .map(|(key, (id, name))| Record {
            online: online.contains(&key),
            id,
            name,
        })
        .collect()
}

/// Remove a display's record. Needs root.
pub fn remove(record: &Record) -> bool {
    unsafe {
        ColorSyncUnregisterDevice(
            kColorSyncDisplayDeviceClass,
            record.id.as_concrete_TypeRef(),
        )
    }
}

/// Log how many records there are, and warn when Sidescreen left many.
pub fn log_records() {
    let records = records();
    let stale = records.iter().filter(|r| r.is_stale()).count();
    if stale > 10 {
        log::warn!(
            "ColorSync keeps {} display records, {stale} of them for Sidescreen displays that are gone; they slow every display change. Remove them with: sudo \"{}\" colorsync clean",
            records.len(),
            std::env::current_exe().unwrap_or_default().display()
        );
    } else {
        log::info!(
            "ColorSync keeps {} display records, {stale} for Sidescreen displays that are gone",
            records.len()
        );
    }
}
