---
default: patch
---

# Agent displays no longer leave macOS re-reading color profiles

Every display change makes macOS read its records of all displays ever connected, once per display online, and retry every 5 seconds until a read finishes within 50 ms. Sidescreen used to leave a new record for each display it made, so after many displays the reads stopped finishing in time, the retries piled up, and apps that ask about displays stuttered. New displays now reuse the numbers of displays that are gone, so the records stay as few as the displays you have at once. To remove the records earlier versions left, run `sudo /Applications/Sidescreen.app/Contents/MacOS/Sidescreen colorsync clean` once. A new agent display also gets its own color profile right away, where it used to keep sRGB.
