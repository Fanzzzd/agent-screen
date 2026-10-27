---
default: minor
---

Recordings… in the menu bar icon's menu opens a local video library. Select a recording to play, pause or seek in the app. Show Pointer toggles a visual overlay without changing the video, sound or playhead. New recordings store timed pointer coordinates; older videos still play when their timelines cannot accurately replay a pointer. The overlay hides during Takeover because user pointer movements were not recorded.

New recordings request HEVC/MP4 when macOS advertises it and use H.264/MP4 when HEVC is unavailable. The recording result reports the selected codec. Hardware encoder use and same-quality size or CPU improvements have not been verified for ScreenCaptureKit. Sound still joins through passthrough export, and the pointer stays outside the encoded video.
