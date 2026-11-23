---
default: patch
---

Typing over all of a masked field, as an expiry date "12/34" retyped as "0234", no longer fails with a report that the app dropped keys. The check after a `type` call now compares only letters, digits and symbols such as emoji, which a mask keeps, without case or accents, and ignores the separators and spaces it adds or removes. The check also catches a key dropped at the end of an address that repeats the typed number earlier, as "12" appended to "12 Main St, Apt " with only "1" shown, which it used to report as typed.
