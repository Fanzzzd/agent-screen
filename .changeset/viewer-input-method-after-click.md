---
default: patch
---

Your Chinese, Japanese or Korean input method composes in the Viewer after you click on the display's picture. In 0.1.24, a click on the picture brought Agent Screen to the front but left the Viewer without the keyboard focus an input method needs, so it composed nothing there: the letters you typed went to the app as typed, such as "nihao" into WeCom's message box, and a number typed a digit instead of picking a candidate. Now the Viewer takes the keyboard focus before Agent Screen comes to the front. The candidates show at the app's text cursor in the Viewer, and the app gets only the text you pick. The Viewer no longer draws a box with the letters you are typing over the picture; the input method's own window shows them.
