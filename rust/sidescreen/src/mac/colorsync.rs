//! ColorSync's records of displays, and what they cost every display change.
//!
//! Each time displays change, WindowServer asks colorsync.displayservices
//! for every online display's profile and waits 50 ms for the answer; a
//! later one is dropped, and it asks again 5 s later, until an answer comes
//! in time. Every display change starts another such loop. For each online
//! display the service has colorsyncd read and parse the whole registry,
//! `REGISTRY`, five times. The registry keeps a record for every display
//! serial macOS has seen, and each record holds the display's name in 39
//! languages: 97% of what gets parsed. So an answer takes about online
//! displays × records × 0.2 ms. Past 50 ms the loops pile up, and
//! WindowServer and ColorSync stay busy (issue #26).
//!
//! Sidescreen reuses serials (`display::Serial`), so it adds a record only
//! per display it holds at once. `sudo Sidescreen colorsync clean` removes
//! the records earlier versions left for displays numbered above `REUSED`
//! and keeps each record's name in English and the user's languages only;
//! macOS leaves a trimmed record as it is when the display comes back. With that, 13 online
//! displays took 29 ms where 4 took 23 ms before. Both need root:
//! ColorSync asks for an administrator to remove a record, and the
//! registry file is root's.

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

/// ColorSync's registry of devices, a binary property list.
const REGISTRY: &str = "/Library/Caches/ColorSync/com.apple.colorsync.devices";

/// A display ColorSync has a record of.
pub struct Record {
    pub id: CFUUID,
    pub name: String,
    pub online: bool,
}

/// Sidescreen displays numbered up to this keep their records when gone:
/// serials are reused, and a record macOS makes anew has all 39 names.
const REUSED: u32 = 16;

impl Record {
    /// One of Sidescreen's displays, not connected now, with a number
    /// above `REUSED`: one of the many earlier versions left.
    pub fn is_stale(&self) -> bool {
        let number = self
            .name
            .rsplit(' ')
            .next()
            .and_then(|n| n.parse::<u32>().ok());
        !self.online && self.name.starts_with("Sidescreen") && number.is_none_or(|n| n > REUSED)
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
            "ColorSync keeps {} display records, {stale} of them for Sidescreen displays that are gone; they slow every display change. Clean them up with: sudo \"{}\" colorsync clean",
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

/// Whether the record of display `id` holds a profile someone chose, which
/// removing it would lose. Unknown counts as yes. Needs root.
pub fn has_chosen_profile(id: &CFUUID) -> bool {
    let Ok(plist::Value::Dictionary(registry)) = plist::Value::from_file(REGISTRY) else {
        return true;
    };
    let key = format!("Device.mntr.{}", text_of(id));
    match registry.get(&key) {
        Some(plist::Value::Dictionary(record)) => record
            .keys()
            .any(|k| k != "DeviceDescriptions" && k != "FactoryProfiles"),
        _ => false,
    }
}

/// Keep each record's names only in English and `languages` ("zh-Hans"
/// keeps every zh_ name). Returns the registry's size before and after.
/// Needs root.
pub fn trim_names(languages: &[String]) -> Result<(u64, u64), String> {
    let before = std::fs::metadata(REGISTRY)
        .map_err(|e| e.to_string())?
        .len();
    let mut registry = plist::Value::from_file(REGISTRY).map_err(|e| e.to_string())?;
    let keep: Vec<String> = std::iter::once("en".to_string())
        .chain(
            languages
                .iter()
                .map(|l| l.split(['-', '_']).next().unwrap_or("").to_string()),
        )
        .filter(|l| !l.is_empty())
        .collect();
    trim(&mut registry, &keep);
    // Beside it, then over it: colorsyncd never reads half a file.
    let partial = format!("{REGISTRY}.sidescreen");
    registry
        .to_file_binary(&partial)
        .map_err(|e| e.to_string())?;
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::metadata(REGISTRY).map_err(|e| e.to_string())?;
    std::fs::set_permissions(
        &partial,
        std::fs::Permissions::from_mode(metadata.permissions().mode()),
    )
    .map_err(|e| e.to_string())?;
    use std::os::unix::fs::MetadataExt;
    std::os::unix::fs::chown(&partial, Some(metadata.uid()), Some(metadata.gid()))
        .map_err(|e| e.to_string())?;
    std::fs::rename(&partial, REGISTRY).map_err(|e| e.to_string())?;
    let after = std::fs::metadata(REGISTRY)
        .map_err(|e| e.to_string())?
        .len();
    Ok((before, after))
}

fn trim(value: &mut plist::Value, keep: &[String]) {
    let plist::Value::Dictionary(dict) = value else {
        return;
    };
    for (key, child) in dict.iter_mut() {
        match child {
            plist::Value::Dictionary(names)
                if key == "DeviceDescriptions" || key == "DeviceModeDescriptions" =>
            {
                let first = names.keys().next().cloned();
                let wanted: Vec<String> = names
                    .keys()
                    .filter(|locale| {
                        let language = locale.split('_').next().unwrap_or("");
                        keep.iter().any(|k| k == language)
                    })
                    .cloned()
                    .collect();
                // A record keeps at least one name.
                let wanted = if wanted.is_empty() {
                    first.into_iter().collect()
                } else {
                    wanted
                };
                names.retain(|locale, _| wanted.contains(locale));
            }
            _ => trim(child, keep),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::trim;

    #[test]
    fn names_keep_english_and_the_users_languages() {
        let mut registry: plist::Value = plist::Value::Dictionary(
            [(
                "Device.mntr.X".to_string(),
                plist::Value::Dictionary(
                    [
                        (
                            "DeviceDescriptions".to_string(),
                            plist::Value::Dictionary(
                                ["en_US", "en_GB", "de_DE", "zh_CN", "zh_TW", "ja_JP"]
                                    .iter()
                                    .map(|l| (l.to_string(), plist::Value::from("Screen")))
                                    .collect(),
                            ),
                        ),
                        (
                            "FactoryProfiles".to_string(),
                            plist::Value::Dictionary(
                                [(
                                    "1".to_string(),
                                    plist::Value::Dictionary(
                                        [(
                                            "DeviceModeDescriptions".to_string(),
                                            plist::Value::Dictionary(
                                                [("ar", "x"), ("fr_FR", "x")]
                                                    .iter()
                                                    .map(|(l, n)| {
                                                        (l.to_string(), plist::Value::from(*n))
                                                    })
                                                    .collect(),
                                            ),
                                        )]
                                        .into_iter()
                                        .collect(),
                                    ),
                                )]
                                .into_iter()
                                .collect(),
                            ),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                ),
            )]
            .into_iter()
            .collect(),
        );
        trim(&mut registry, &["en".into(), "zh".into()]);
        let record = registry.as_dictionary().unwrap()["Device.mntr.X"]
            .as_dictionary()
            .unwrap();
        let names: Vec<_> = record["DeviceDescriptions"]
            .as_dictionary()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(names, ["en_US", "en_GB", "zh_CN", "zh_TW"]);
        // Neither language there: the first name stays.
        let modes = record["FactoryProfiles"].as_dictionary().unwrap()["1"]
            .as_dictionary()
            .unwrap()["DeviceModeDescriptions"]
            .as_dictionary()
            .unwrap();
        assert_eq!(modes.keys().collect::<Vec<_>>(), ["ar"]);
    }
}
