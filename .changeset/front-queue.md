---
default: minor
---

When an agent works in a window of an app you're using in another window, such as your browser, an action that would reach your window or take its focus no longer just fails. Other shortcuts, right and double clicks, a select, and a click on a control that takes no accessibility press, as a page's buttons in Dia, now wait in the Input Queue with nothing sent. They need no approval and never use your pointer: as soon as you switch to another app, the action goes in the normal background way, while the agent waits for it. A click doesn't run if what was under it changed meanwhile. After 30 seconds the display's card in Peek and the Viewer header say "Waits for you to leave Dia", with no notification. Before, a click on a Dia page's button went in as an accessibility press that the page ignored, and the agent was told it might not have worked.
