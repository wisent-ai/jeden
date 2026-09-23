// Continuation of bridge-core.mjs; the browser service concatenates both files.
const handleTabAction = async (action, input, state) => {
  if (action === "list") return { ok: true, action, tabs: await listTabs(), currentTab: state.currentTab ?? null };
  if (action === "open" || action === "new") {
    const tab = await openTab(input.url ?? "about:blank");
    state.currentTab = tab.id;
    await saveState(state);
    return { ok: true, action: "open", tab };
  }
  const tab = await resolveTab(input, state);
  if (!tab) throw new Error("no browser tab is available");
  if (action === "focus" || action === "activate") {
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
  throw new Error(`unsupported browser_tab action: ${action}`);
};

const handlePageAction = async (action, input, state) => {
  let tab = await resolveTab(input, state);
  if (!tab) tab = await openTab("about:blank");
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
        const url = String(input.url ?? "").trim();
        if (!url) throw new Error("url is required");
        await client.send("Page.navigate", { url });
        await waitReady(client);
        value = { url: await evaluate(client, "location.href"), title: await evaluate(client, "document.title") };
        break;
      }
      case "click": {
        if (input.selector) {
          value = await evaluate(client, `(() => { const element = document.querySelector(${jsString(input.selector)}); if (!element) throw new Error("selector not found"); element.click(); return true; })()`);
        } else {
          const x = Number(input.x);
          const y = Number(input.y);
          if (!Number.isFinite(x) || !Number.isFinite(y)) throw new Error("selector or finite x/y coordinates are required");
          await client.send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", clickCount: 1 });
          await client.send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", clickCount: 1 });
          value = true;
        }
        break;
      }
      case "type":
      case "fill": {
        const selector = String(input.selector ?? "").trim();
        if (!selector) throw new Error("selector is required");
        const text = String(input.text ?? input.value ?? "");
        value = await evaluate(client, `(() => { const element = document.querySelector(${jsString(selector)}); if (!element) throw new Error("selector not found"); element.focus(); element.value = ${jsString(text)}; element.dispatchEvent(new Event("input", { bubbles: true })); element.dispatchEvent(new Event("change", { bubbles: true })); return element.value; })()`);
        break;
      }
      case "press": {
        const key = String(input.key ?? "Enter");
        await client.send("Input.dispatchKeyEvent", { type: "keyDown", key });
        await client.send("Input.dispatchKeyEvent", { type: "keyUp", key });
        value = true;
        break;
      }
      case "evaluate":
      case "eval": {
        const expression = String(input.expression ?? input.code ?? "").trim();
        if (!expression) throw new Error("expression is required");
        value = await evaluate(client, expression, true);
        break;
      }
      case "wait": {
        if (input.selector) await waitSelector(client, String(input.selector));
        else await sleep(Math.max(0, Number(input.ms ?? input.milliseconds ?? 250)));
        value = true;
        break;
      }
      case "scroll": {
        const x = Number(input.x ?? input.deltaX ?? 0);
        const y = Number(input.y ?? input.deltaY ?? 600);
        value = await evaluate(client, `(() => { scrollBy(${Number.isFinite(x) ? x : 0}, ${Number.isFinite(y) ? y : 600}); return { x: scrollX, y: scrollY }; })()`);
        break;
      }
      case "inspect":
      case "observe":
      case "snapshot": {
        value = await evaluate(client, pageSnapshotExpression);
        break;
      }
      case "screenshot": {
        const format = input.format === "jpeg" ? "jpeg" : "png";
        const params = { format, captureBeyondViewport: input.fullPage !== false };
        if (format === "jpeg") params.quality = Math.max(0, Math.min(Number(input.quality ?? 85), 100));
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

try {
  const request = await readStdin();
  const input = request.input && typeof request.input === "object" ? request.input : request;
  const action = String(request.action ?? input.action ?? "").trim().toLowerCase();
  if (!action) throw new Error("action is required");
  await ensureBrowser();
  const state = await readState();
  const tool = String(input.tool ?? request.tool ?? "");
  const result = tool === "browser_tab" || ["list", "open", "new", "focus", "activate", "close"].includes(action)
    ? await handleTabAction(action, input, state)
    : await handlePageAction(action, input, state);
  const latestState = await readState();
  if (Number.isInteger(latestState.browserPid)) result.browserPid = latestState.browserPid;
  process.stdout.write(JSON.stringify(result));
} catch (error) {
  process.stdout.write(JSON.stringify({ ok: false, error: error instanceof Error ? error.message : String(error) }));
}
