// Agent Screen's way into the browser the user runs, signed in as them
// (ADR 0012). Agent Screen's app holds the other end, through the native
// messaging host `com.agentscreen.browser`, and speaks the subset of the
// DevTools protocol it speaks over its private pipe (ADR 0010):
// Target.getTargets, Target.attachToTarget, Target.detachFromTarget, and
// commands for an attached page, whose session id is its target id.
// The browser shows its "started debugging this browser" bar while a page
// is attached; a page is let go after a while without input, and the bar
// with it.

const HOST = "com.agentscreen.browser";
const IDLE_MS = 20000;

let port = null;
// Target id to the timer that lets it go.
const attached = new Map();
// The user pressed Cancel on the bar: no more attaching until the browser
// starts again. Session storage outlives this worker, not the browser.
const canceled = () =>
  chrome.storage.session.get("canceled").then((s) => s.canceled === true);

function post(message) {
  try {
    port?.postMessage(message);
  } catch {
    port = null;
  }
}

function keep(targetId) {
  clearTimeout(attached.get(targetId));
  attached.set(
    targetId,
    setTimeout(() => {
      attached.delete(targetId);
      chrome.debugger.detach({ targetId }).catch(() => {});
      post({ method: "Target.detachedFromTarget", params: { sessionId: targetId, reason: "idle" } });
    }, IDLE_MS),
  );
}

// The page shown in each window, with where its window is in global
// points, so the app attaches only to the page under the agent's point.
async function targets() {
  const all = await chrome.debugger.getTargets();
  const infos = [];
  for (const t of all) {
    if (t.type !== "page" || t.tabId === undefined) continue;
    const tab = await chrome.tabs.get(t.tabId).catch(() => null);
    if (!tab?.active) continue;
    const w = await chrome.windows.get(tab.windowId).catch(() => null);
    if (!w || w.state === "minimized") continue;
    infos.push({
      targetId: t.id,
      type: "page",
      attached: attached.has(t.id),
      window: { x: w.left, y: w.top, width: w.width, height: w.height },
    });
  }
  return { targetInfos: infos };
}

async function handle({ method, params = {}, sessionId }) {
  if (sessionId) {
    if (!attached.has(sessionId)) throw new Error("the page is no longer attached");
    keep(sessionId);
    return (await chrome.debugger.sendCommand({ targetId: sessionId }, method, params)) ?? {};
  }
  switch (method) {
    case "Target.getTargets":
      return targets();
    case "Target.attachToTarget": {
      if (await canceled()) {
        throw new Error("the user canceled Agent Screen's access from the browser's bar; it comes back when the browser restarts");
      }
      const { targetId } = params;
      if (!attached.has(targetId)) {
        await chrome.debugger.attach({ targetId }, "1.3");
      }
      keep(targetId);
      return { sessionId: targetId };
    }
    case "Target.detachFromTarget": {
      const targetId = params.sessionId;
      clearTimeout(attached.get(targetId));
      attached.delete(targetId);
      await chrome.debugger.detach({ targetId }).catch(() => {});
      return {};
    }
    default:
      throw new Error(`${method} isn't available through the extension`);
  }
}

function connect() {
  if (port) return;
  try {
    port = chrome.runtime.connectNative(HOST);
  } catch {
    port = null;
    return;
  }
  port.onMessage.addListener((message) => {
    handle(message).then(
      (result) => post({ id: message.id, result }),
      (error) => post({ id: message.id, error: { message: String(error?.message ?? error) } }),
    );
  });
  port.onDisconnect.addListener(() => {
    port = null;
    // The app quit: nothing drives the pages any more.
    for (const [targetId, timer] of attached) {
      clearTimeout(timer);
      chrome.debugger.detach({ targetId }).catch(() => {});
    }
    attached.clear();
  });
}

chrome.debugger.onEvent.addListener((source, method, params) => {
  if (source.targetId && attached.has(source.targetId)) {
    post({ method, params, sessionId: source.targetId });
  }
});

chrome.debugger.onDetach.addListener((source, reason) => {
  if (!source.targetId) return;
  clearTimeout(attached.get(source.targetId));
  attached.delete(source.targetId);
  if (reason === "canceled_by_user") chrome.storage.session.set({ canceled: true });
  post({ method: "Target.detachedFromTarget", params: { sessionId: source.targetId, reason } });
});

// The app may start after the browser: look for it every half minute.
chrome.alarms.create("connect", { periodInMinutes: 0.5 });
chrome.alarms.onAlarm.addListener(connect);
chrome.runtime.onStartup.addListener(connect);
chrome.runtime.onInstalled.addListener(connect);
connect();
