# An extension in the user's browser

ADR 0010 gives a pipe only to a Chromium app that `launch_app` starts. Agents often need the browser the user already runs, signed in as them, and a fresh profile has none of those logins (#104). A drag there had no way in: posted events reach the page with no button held, and Agent Screen won't press the session's button (ADR 0010).

Agent Screen ships a Chromium extension. It uses `chrome.debugger`, which sends DevTools commands to a page from inside the browser, the same `Input.dispatchMouseEvent` and `Input.dispatchDragEvent` the pipe uses. Nothing reaches WindowServer, so the user's pointer and button stay theirs.

We looked at how others reach a logged-in browser. Claude in Chrome, the Codex extension, Playwright's `--extension` mode and browsermcp all use an extension with `chrome.debugger`. Chrome 144's remote-debugging toggle with `--autoConnect` needs no extension, but works only in Chrome and Edge, and brings a consent dialog to the front for each connection, on the user's screen. cua-driver and Peekaboo find no way to make one process see the button held either.

## How it connects

- The extension (`apps/macos/resources/browser-extension`) connects to the native messaging host `com.agentscreen.browser`. The host is Agent Screen's own binary: a browser starts it with the extension's origin as its argument.
- At launch, the user's Agent Screen writes the host's manifest into the `NativeMessagingHosts` folder of each Chromium browser the user has: Chrome, Chromium, Edge, Brave, Vivaldi, Arc and Dia. It admits only the extension's id, fixed by the key in its manifest. Test and development instances don't write it.
- The host passes messages between the browser and the app's browser socket, beside the control socket and like it readable only by the user. The app takes a connection only from its own binary whose parent, as the kernel reports it, is a Chromium browser. The browser's pid names the channel.
- The extension speaks the part of the DevTools protocol the pipe uses: `Target.getTargets`, `Target.attachToTarget` and commands for an attached page, with the target id as the session id. So a drag goes the same way through either, and `devtools::route` needs no second path.
- `Target.getTargets` lists only the page shown in each window, with the window's frame. Agent Screen attaches only to the page whose window is the one under the agent's point, never to the user's other tabs.
- An app with a pipe keeps it: the pipe shows no bar.

## What the user sees

- They add the extension once: Settings, Your Browser, Add… copies it to `~/Library/Application Support/Agent Screen/Browser Extension` and says how to load it unpacked. Each launch refreshes that copy, so it follows the app's version. A Chrome Web Store listing would replace the Developer mode step.
- While a page is attached, Chromium shows "'Agent Screen' started debugging this browser" in all its windows. Only a policy-installed extension or `--silent-debugger-extension-api` hides it, and neither fits a user's own browser. The extension lets a page go 20 s after the last command, and the bar goes with it. The bar is the user's sign that an agent is working in their browser.
- The bar's Cancel stops the extension until the browser restarts. The agent hears that the user stopped it, and to call `request_help`.

## Consequences

- Drags work in the user's own Chrome, Dia, Arc, Edge and Brave once the extension is added, in the window as it is, signed in.
- The bar pushes the page down while it shows. The agent's point maps by the web area accessibility reported before the attach, so the first drag lands where the agent aimed. The agent's next screenshot shows the moved page.
- A browser whose policy blocks `chrome.debugger`, as Comet does, can't use the extension. Its drags fail as before.
- Safari and other WebKit apps still have no way in.
