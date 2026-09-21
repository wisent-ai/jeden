// Installed by `jeden context install --omp`. Do not edit by hand: the next
// install overwrites this file from the Jeden binary that produced it.
//
// Omp sessions get Jeden's context advisor as a normal tool through Omp's own
// custom-tool directory, so no Omp source is patched and the recommendation
// logic stays in one place — the Jeden CLI this file calls.
import type { CustomToolFactory } from "@oh-my-pi/pi-coding-agent";

const JEDEN_BIN = "__JEDEN_BIN__";

const factory: CustomToolFactory = (pi) => {
  return {
    name: "context_recommend",
    label: "Context Recommend",
    description:
      "Ask Jeden's context advisor what to read for a task before searching. Returns ranked locators (file:line ranges, session ids, cited repository paths) from the documentation corpus, Jeden's memory, the Transcript Lake archive, and the Wisent ground-truth index, plus the state of every source that answered nothing. Prefer this over guessing which file holds the answer.",
    loadMode: "essential",
    parameters: pi.zod.object({
      query: pi.zod.string().describe("The task or question to find context for"),
      limit: pi.zod
        .number()
        .int()
        .positive()
        .optional()
        .describe("Maximum recommendations to return"),
      sources: pi.zod
        .string()
        .optional()
        .describe(
          "Comma-separated subset of docs, ground-truth, memory, transcripts, or all",
        ),
      timeoutMs: pi.zod
        .number()
        .int()
        .positive()
        .optional()
        .describe("Per-source deadline in milliseconds"),
    }),

    async execute(_toolCallId, params, onUpdate, _ctx, signal) {
      const argv = ["context", "recommend", params.query, "--json"];
      if (params.limit !== undefined) argv.push("--limit", String(params.limit));
      if (params.sources !== undefined) argv.push("--source", params.sources);
      if (params.timeoutMs !== undefined) argv.push("--timeout-ms", String(params.timeoutMs));

      onUpdate?.({
        content: [{ type: "text", text: `Asking Jeden for context on: ${params.query}` }],
        details: { query: params.query, phase: "asking" },
      });

      const result = await pi.exec(JEDEN_BIN, argv, { signal });
      if (result.killed) throw new Error("jeden context recommend was cancelled");
      if (result.code !== 0) {
        throw new Error(result.stderr.trim() || "jeden context recommend failed");
      }

      let advice: {
        query: string;
        recommendations: {
          source: string;
          title: string;
          locator: string;
          score: number;
          snippet: string;
        }[];
        sources: { source: string; available: boolean; detail: string }[];
      };
      try {
        advice = JSON.parse(result.stdout);
      } catch {
        throw new Error("jeden context recommend returned malformed JSON");
      }

      const lines = advice.recommendations.map(
        (hit) => `- [${hit.source}] ${hit.locator} — ${hit.title}\n  ${hit.snippet.replace(/\n/g, " ")}`,
      );
      const missing = advice.sources
        .filter((source) => !source.available)
        .map((source) => `- ${source.source}: ${source.detail}`);
      const text = [
        lines.length > 0
          ? `Read these before searching:\n${lines.join("\n")}`
          : "No documentation, memory, transcript or ground-truth match was found.",
        missing.length > 0 ? `Sources that answered nothing:\n${missing.join("\n")}` : "",
      ]
        .filter((part) => part.length > 0)
        .join("\n\n");

      return {
        content: [{ type: "text", text }],
        details: advice,
      };
    },
  };
};

export default factory;
