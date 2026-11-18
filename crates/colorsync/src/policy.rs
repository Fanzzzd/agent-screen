//! Which display records reclaim removes, and the names it keeps, decided
//! on the registry's contents alone (ADR 0009).

use plist::{Dictionary, Value};
use std::collections::BTreeSet;

/// Keys of a record that holds only what macOS made for the display.
const FACTORY_KEYS: [&str; 2] = ["DeviceDescriptions", "FactoryProfiles"];
/// Keys whose value is a display name per locale.
const NAME_KEYS: [&str; 2] = ["DeviceDescriptions", "DeviceModeDescriptions"];
const RECORD_PREFIX: &str = "Device.mntr.";

/// The registry isn't what reclaim knows how to read: it changes nothing.
#[derive(Debug, PartialEq, Eq)]
pub struct UnknownShape(pub String);

/// The records to unregister, by UUID, in registry order.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub remove: Vec<String>,
    pub records: usize,
}

fn records(registry: &Value) -> Result<&Dictionary, UnknownShape> {
    let dict = registry
        .as_dictionary()
        .ok_or_else(|| UnknownShape("the registry isn't a dictionary".into()))?;
    if let Some(key) = dict.keys().find(|k| !k.starts_with("Device.")) {
        return Err(UnknownShape(format!("unknown key {key}")));
    }
    Ok(dict)
}

/// The display UUID of a record key, upper case, if the key is a display's.
pub fn uuid_of(key: &str) -> Option<String> {
    let uuid = key.strip_prefix(RECORD_PREFIX)?;
    is_uuid(uuid).then(|| uuid.to_ascii_uppercase())
}

/// Whether `text` is a UUID in its usual 8-4-4-4-12 hex form.
pub fn is_uuid(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Whether a record holds only factory data: its names and the profile
/// macOS made. Anything else, such as a profile someone chose or a key this
/// version doesn't know, keeps it.
fn factory_only(record: &Value) -> bool {
    let Some(record) = record.as_dictionary() else {
        return false;
    };
    record.keys().all(|k| FACTORY_KEYS.contains(&k.as_str()))
        && record
            .get("DeviceDescriptions")
            .is_some_and(|d| d.as_dictionary().is_some())
}

/// The display records to remove: those of displays not `online`, not in
/// `keep`, holding only factory data.
pub fn plan(
    registry: &Value,
    online: &BTreeSet<String>,
    keep: &BTreeSet<String>,
) -> Result<Plan, UnknownShape> {
    let records = records(registry)?;
    let remove = records
        .iter()
        .filter_map(|(key, record)| {
            let uuid = uuid_of(key)?;
            (!online.contains(&uuid) && !keep.contains(&uuid) && factory_only(record))
                .then_some(uuid)
        })
        .collect();
    Ok(Plan {
        remove,
        records: records.keys().filter(|k| uuid_of(k).is_some()).count(),
    })
}

/// Keep each record's names only in `languages` ("en", "zh"), at least one
/// per record. Whether anything changed.
pub fn trim_names(registry: &mut Value, languages: &BTreeSet<String>) -> bool {
    let mut changed = false;
    trim(registry, languages, &mut changed);
    changed
}

fn trim(value: &mut Value, languages: &BTreeSet<String>, changed: &mut bool) {
    let Some(dict) = value.as_dictionary_mut() else {
        return;
    };
    for (key, child) in dict.iter_mut() {
        match child {
            Value::Dictionary(names) if NAME_KEYS.contains(&key.as_str()) => {
                let wanted: Vec<String> = names
                    .keys()
                    .filter(|locale| languages.contains(language_of(locale)))
                    .cloned()
                    .collect();
                let wanted = if wanted.is_empty() {
                    names.keys().next().cloned().into_iter().collect()
                } else {
                    wanted
                };
                if wanted.len() < names.len() {
                    names.retain(|locale, _| wanted.contains(locale));
                    *changed = true;
                }
            }
            _ => trim(child, languages, changed),
        }
    }
}

/// "zh" of "zh_CN", "zh-Hans-HK" or "zh".
pub fn language_of(locale: &str) -> &str {
    locale.split(['_', '-']).next().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "11111111-2222-3333-4444-555555555555";
    const B: &str = "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE";
    const C: &str = "0A0A0A0A-0B0B-0C0C-0D0D-0E0E0E0E0E0E";
    const D: &str = "99999999-8888-7777-6666-555555555555";

    fn names(locales: &[&str]) -> Value {
        Value::Dictionary(
            locales
                .iter()
                .map(|l| (l.to_string(), Value::from("Screen")))
                .collect(),
        )
    }

    fn record(extra: Option<&str>) -> Value {
        let mut dict = Dictionary::new();
        dict.insert(
            "DeviceDescriptions".into(),
            names(&["en_US", "de_DE", "zh_CN", "ja_JP"]),
        );
        let mut factory = Dictionary::new();
        let mut one = Dictionary::new();
        one.insert("DeviceModeDescriptions".into(), names(&["ar", "fr_FR"]));
        one.insert("DeviceProfileURL".into(), Value::from("/x.icc"));
        factory.insert("1".into(), Value::Dictionary(one));
        dict.insert("FactoryProfiles".into(), Value::Dictionary(factory));
        if let Some(key) = extra {
            dict.insert(key.into(), Value::from(true));
        }
        Value::Dictionary(dict)
    }

    fn registry(entries: &[(&str, Value)]) -> Value {
        Value::Dictionary(
            entries
                .iter()
                .map(|(uuid, record)| (format!("Device.mntr.{uuid}"), record.clone()))
                .collect(),
        )
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn removes_only_offline_factory_records_not_kept() {
        let registry = registry(&[
            (A, record(None)),
            (B, record(None)),
            (C, record(Some("CustomProfiles"))),
            (D, record(None)),
        ]);
        let plan = plan(&registry, &set(&[A]), &set(&[D])).unwrap();
        assert_eq!(plan.remove, vec![B.to_string()]);
        assert_eq!(plan.records, 4);
    }

    #[test]
    fn keeps_records_of_unknown_shape_and_refuses_unknown_registries() {
        let odd = Value::from("not a record");
        let registry = registry(&[(A, odd)]);
        assert!(
            plan(&registry, &set(&[]), &set(&[]))
                .unwrap()
                .remove
                .is_empty()
        );
        let mut other = Dictionary::new();
        other.insert("Version".into(), Value::from(2));
        assert!(plan(&Value::Dictionary(other), &set(&[]), &set(&[])).is_err());
        assert!(plan(&Value::from(1), &set(&[]), &set(&[])).is_err());
    }

    #[test]
    fn printer_records_are_never_touched() {
        let mut dict = Dictionary::new();
        dict.insert(format!("Device.prtr.{A}"), record(None));
        let plan = plan(&Value::Dictionary(dict), &set(&[]), &set(&[])).unwrap();
        assert!(plan.remove.is_empty());
    }

    #[test]
    fn names_keep_the_languages_and_one_at_least() {
        let mut registry = registry(&[(A, record(None))]);
        assert!(trim_names(&mut registry, &set(&["en", "zh"])));
        let record = registry.as_dictionary().unwrap()[&format!("Device.mntr.{A}")]
            .as_dictionary()
            .unwrap();
        let kept: Vec<_> = record["DeviceDescriptions"]
            .as_dictionary()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(kept, ["en_US", "zh_CN"]);
        let modes = record["FactoryProfiles"].as_dictionary().unwrap()["1"]
            .as_dictionary()
            .unwrap()["DeviceModeDescriptions"]
            .as_dictionary()
            .unwrap();
        assert_eq!(modes.keys().collect::<Vec<_>>(), ["ar"]);
        // Trimmed already: nothing to write.
        assert!(!trim_names(&mut registry, &set(&["en", "zh"])));
    }

    #[test]
    fn uuids_are_checked() {
        assert!(is_uuid(A));
        assert!(!is_uuid("1111-2222"));
        assert!(!is_uuid("11111111-2222-3333-4444-55555555555G"));
        assert!(!is_uuid("--keep"));
        assert_eq!(
            uuid_of(&format!("Device.mntr.{}", B.to_lowercase())),
            Some(B.into())
        );
        assert_eq!(language_of("zh-Hans-HK"), "zh");
    }
}
