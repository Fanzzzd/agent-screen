---
default: patch
---

Your Chinese, Japanese or Korean input method works in the Viewer during a Takeover (#117). Before, the Viewer sent every key straight to the app, so the input method never saw one and you could type only letters. Now the input method composes in the Viewer: the text you are typing shows at the app's text cursor, underlined, with the input method's candidates under it, and the app gets only the text you pick. With a keyboard layout such as ABC, keys go to the app as before. Shortcuts with Command or Control, and keys that type no text, such as the arrows, Return and Escape, also go to the app unless you are in the middle of composing.
