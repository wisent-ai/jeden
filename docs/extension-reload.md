# An extension tool can reload this session's extensions

A native extension is discovered and activated when a session starts. Its
tools, hooks and commands are registered then, from the material on disk at
that moment. Anything installed later belonged to a later session.

That is a problem for a tool whose whole job is to install extension material:
it could finish successfully and then report only that the runtime it had
installed is not the runtime it is running. The single way to load it was the
`/…` command a person types.

## The request

An extension tool's execution context carries `requestReload()`:

```js
api.registerTool({
  name: 'install_policy',
  description: 'install a policy release and load it here',
  parameters: api.zod.object({}),
  execute: async (_id, input, _update, context) => {
    const installed = await install(input);
    await context.requestReload();
    return installed;
  },
});
```

The host runs in a short-lived process, so it cannot touch the registry the
session holds in memory. `requestReload()` records the request as
`.jeden/runtime/extensions/reload-request.json`, and the session consumes it
immediately after the tool answers: it rebuilds the registry with a new
generation, re-activating every extension source and re-materializing tools,
hooks and commands.

## What the caller sees

When a reload was requested, the tool's answer is wrapped:

```json
{
  "result": { "…": "the tool's own answer" },
  "extensionReload": {
    "reloaded": true,
    "generation": 4,
    "activeExtensions": 3,
    "tools": 11,
    "hooks": 6
  }
}
```

A failed reload reports `{"reloaded": false, "error": "…"}` beside the same
answer, so a tool that asked for one can never look successful while the
reload did not happen. A tool that never calls `requestReload()` gets its
answer unchanged and no reload runs.

The request is consumed per call: one tool call reloads at most once, and a
later call can ask again.

`tests/extension_reload.rs` drives this through the real host process and the
real tool dispatcher.
