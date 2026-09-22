// How this extension speaks to the agent: the protocol shapes and their
// parser, the transport carrying them over the child process's own streams,
// and the client that turns them into what the editor reacts to.
//
// Grouped here so the extension source folder keeps to five entries.

export * from "./protocol.js";
export * from "./transport.js";
export * from "./client.js";
