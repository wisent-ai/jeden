// Continuation of bridge-core.mjs; the browser service concatenates both files.

// A tab opened without a page to show holds the empty page.
const BLANK_PAGE = "about:blank";

const evaluate = async (client, expression) => {
  const result = await client.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true, userGesture: true });
  const details = result.exceptionDetails;
  if (details) throw new Error(details.exception === undefined ? details.text : details.exception.description);
  return result.result.value;
};
const jsString = value => JSON.stringify(String(value));
// An element appears when the page renders it: the page's own MutationObserver
// reports it, and the turn's own cancellation ends a request that should stop.
const untilSelector = (client, selector) => evaluate(client, `new Promise(resolve => {
  const selector = ${jsString(selector)};
  if (document.querySelector(selector)) { resolve(true); return; }
  const observer = new MutationObserver(() => {
    if (document.querySelector(selector)) { observer.disconnect(); resolve(true); }
  });
  observer.observe(document, { childList: true, subtree: true, attributes: true });
})`);
// The page's title, address and text, and its first `limit` visible controls,
// every text cut to `maxChars` characters: both amounts are the caller's.
const pageSnapshotExpression = (limit, maxChars) => `(() => {
  const head = text => Array.from(text).filter((_, index) => index < ${maxChars}).join("");
  // The browser answers whether an element is rendered and visible
  // (Element.checkVisibility, which covers display and visibility); an
  // element with no box takes no click.
  const visible = element => element.checkVisibility({ visibilityProperty: true }) && element.getClientRects().length;
  const elements = [...document.querySelectorAll("a,button,input,textarea,select,[role],[contenteditable=true]")]
    .filter(visible).filter((_, index) => index < ${limit}).map((element, index) => ({
      index,
      tag: element.tagName.toLowerCase(),
      role: element.getAttribute("role"),
      text: head(element.innerText.trim()),
      value: element.value,
      label: element.getAttribute("aria-label"),
      title: element.getAttribute("title"),
      id: element.id,
      name: element.getAttribute("name"),
      type: element.getAttribute("type"),
      href: element.href,
      disabled: Boolean(element.disabled),
    }));
  return { title: document.title, url: location.href, text: document.body === null ? null : head(document.body.innerText), elements };
})()`;
/** A positive whole number the caller stated under `key`, refused by name otherwise. */
const requiredCount = (input, key, action) => {
  const value = input[key];
  if (!Number.isSafeInteger(value) || !value || Math.abs(value) !== value) throw new Error(`${action} requires ${key}, a positive whole number`);
  return value;
};

/** The text the caller stated under `key`, refused by name when it is missing or empty. */
const requiredText = (input, key, action) => {
  const value = input[key];
  if (typeof value !== "string" || !value.trim()) throw new Error(`${action} requires ${key}, a non-empty string`);
  return value.trim();
};
/** The one value stated under any of `keys`, refused when none or several are. */
const oneOf = (input, keys, action) => {
  const stated = keys.filter(key => input[key] !== undefined);
  const [key, ...again] = stated;
  if (key === undefined) throw new Error(`${action} requires one of ${keys.join(", ")}`);
  if (again.length) throw new Error(`${action} takes one of ${keys.join(", ")}, not ${stated.join(" and ")}`);
  if (typeof input[key] !== "string") throw new Error(`${action} ${key} must be a string`);
  return input[key];
};
/** A coordinate the caller stated under `key`, or undefined when it is absent. */
const coordinate = (input, key, action) => {
  const value = input[key];
  if (value === undefined) return undefined;
  if (typeof value !== "number" || !Number.isFinite(value)) throw new Error(`${action} ${key} must be a finite number`);
  return value;
};

const handleTabAction = async (action, input, state) => {
  if (action === "list") return { ok: true, action, tabs: await listTabs(), currentTab: state.currentTab };
  if (action === "open") {
    const tab = input.url === undefined ? await openTab(BLANK_PAGE) : await openTab(requiredText(input, "url", "browser_tab open"));
    state.currentTab = tab.id;
    await saveState(state);
    return { ok: true, action: "open", tab };
  }
  const tab = await resolveTab(input, state);
  if (!tab) throw new Error("no browser tab is available");
  if (action === "focus") {
    await activateTab(tab.id);
    state.currentTab = tab.id;
    await saveState(state);
    return { ok: true, action: "focus", tab };
  }
  if (action === "close") {
    await closeTab(tab.id);
    if (state.currentTab === tab.id) delete state.currentTab;
    await saveState(state);
    return { ok: true, action, tab };
  }
  throw new Error(`unsupported browser_tab action: ${action}; browser_tab takes list, open, focus or close`);
};

const handlePageAction = async (action, input, state) => {
  let tab = await resolveTab(input, state);
  if (!tab) tab = await openTab(BLANK_PAGE);
  state.currentTab = tab.id;
  await saveState(state);
  const targets = await requestJson("/json/list");
  const target = targets.find(item => item.id === tab.id);
  if (!target?.webSocketDebuggerUrl) throw new Error(`tab ${tab.id} has no CDP endpoint`);
  const client = new CdpClient(target.webSocketDebuggerUrl);
  await client.connect();
  try {
    await client.send("Page.enable");
    await client.send("Runtime.enable");

    let value;
    switch (action) {
      case "navigate":
      case "goto": {
        const url = requiredText(input, "url", action);
        // The page's own DOMContentLoaded event ends this request; a failed
        // navigation is reported with Chromium's errorText.
        const loaded = client.next("Page.domContentEventFired");
        // A failed or same-document navigation fires no load event: the wait
        // is withdrawn, so closing the socket later does not reject it.
        if (navigation.errorText || !navigation.loaderId) client.events.delete("Page.domContentEventFired");
        if (navigation.errorText) throw new Error(`navigation to ${url} failed: ${navigation.errorText}`);
        if (navigation.loaderId) await loaded;
        value = { url: await evaluate(client, "location.href"), title: await evaluate(client, "document.title") };
        break;
      }
      case "click": {
        // A selector clicks that element; coordinates click whatever the page shows there.
        const target = input.selector === undefined
          ? `document.elementFromPoint(${coordinate(input, "x", action)}, ${coordinate(input, "y", action)})`
          : `document.querySelector(${jsString(requiredText(input, "selector", action))})`;
        value = await evaluate(client, `(() => { const element = ${target}; if (!element) throw new Error("no element there to click"); element.click(); return true; })()`);
        break;
      }
      case "type":
      case "fill": {
        const selector = requiredText(input, "selector", action);
        const text = oneOf(input, ["text", "value"], action);
        value = await evaluate(client, `(() => { const element = document.querySelector(${jsString(selector)}); if (!element) throw new Error("selector not found"); element.focus(); element.value = ${jsString(text)}; element.dispatchEvent(new Event("input", { bubbles: true })); element.dispatchEvent(new Event("change", { bubbles: true })); return element.value; })()`);
        break;
      }
      case "press": {
        const key = requiredText(input, "key", action);
        await client.send("Input.dispatchKeyEvent", { type: "keyDown", key });
        await client.send("Input.dispatchKeyEvent", { type: "keyUp", key });
        value = true;
        break;
      }
      case "evaluate":
      case "eval": {
        const expression = oneOf(input, ["expression", "code"], action).trim();
        if (!expression) throw new Error(`${action} requires a non-empty expression`);
        value = await evaluate(client, expression);
        break;
      }
      case "wait": {
        const selector = requiredText(input, "selector", "wait (the page is observed until it renders that element; a fixed pause is not offered)");
        await untilSelector(client, selector);
        value = true;
        break;
      }
      case "scroll": {
        // Each stated axis moves by its amount in pixels; an axis left out does not move.
        const by = { left: coordinate(input, "x", action), top: coordinate(input, "y", action) };
        if (by.left === undefined && by.top === undefined) throw new Error("scroll requires x, y or both: the pixels to move along each axis");
        value = await evaluate(client, `(() => { scrollBy(${JSON.stringify(by)}); return { x: scrollX, y: scrollY }; })()`);
        break;
      }
      case "inspect":
      case "observe":
      case "snapshot": {
        value = await evaluate(client, pageSnapshotExpression(requiredCount(input, "limit", action), requiredCount(input, "maxChars", action)));
        break;
      }
      case "screenshot": {
        // The image format and its quality are Chromium's to check (Page.captureScreenshot).
        const format = requiredText(input, "format", "screenshot (png, jpeg or webp)");
        const params = { format, captureBeyondViewport: input.fullPage !== false };
        if (input.quality !== undefined) params.quality = input.quality;
        const capture = await client.send("Page.captureScreenshot", params);
        return { ok: true, action, tab: tab.id, format, data: capture.data };
      }
      default:
        throw new Error(`unsupported browser action: ${action}`);
    }
    return { ok: true, action, tab: tab.id, value };
  } finally {
    client.close();
  }
};

// The browser this call found or started; every answer, a failure included,
// names it, so the service that called the bridge keeps ownership of it.
let owned = null;
try {
  const request = await readStdin();
  if (typeof request.action !== "string" || !request.action.trim()) {
    throw new Error("the bridge request names no action: its action field must be the browser verb the service resolved");
  }
  if (!request.input || typeof request.input !== "object") {
    throw new Error("the bridge request carries no input object: the service sends the tool's input with every request");
  }
  const action = request.action.trim().toLowerCase();
  owned = await ensureBrowser();
  // The service names the tool every request is for: browser_tab manages
  // tabs, browser_action and browser_screenshot act on a page.
  const result = request.tool === "browser_tab"
    ? await handleTabAction(action, request.input, owned)
    : await handlePageAction(action, request.input, owned);
  if (Number.isInteger(owned.browserPid)) result.browserPid = owned.browserPid;
  process.stdout.write(JSON.stringify(result));
} catch (error) {
  const failed = { ok: false, error: error instanceof Error ? error.message : String(error) };
  if (Number.isInteger(owned?.browserPid)) failed.browserPid = owned.browserPid;
  process.stdout.write(JSON.stringify(failed));
}
