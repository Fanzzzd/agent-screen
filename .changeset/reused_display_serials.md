---
default: patch
---

# Many agent displays no longer leave macOS re-reading color profiles

Every display change makes macOS read its color records of all displays ever connected, once per display online, and retry every 5 seconds until a read finishes within 50 ms. Sidescreen used to leave a new record for each display it made, so after many displays the reads stopped finishing in time, the retries piled up, and apps that ask about displays stuttered. New displays now reuse the numbers of displays that are gone. Smooth Display Changes, in the setup window, removes the records earlier versions left and keeps each record's names in English and your languages, after which 13 displays online read in 19 ms. A new agent display also gets its own color profile right away, where it used to keep sRGB.
