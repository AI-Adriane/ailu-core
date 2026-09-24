import { request as httpRequest } from "node:http";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { createGraph, rustEngineAvailable, serveInspector } from "./index.js";

/** Read an SSE stream, collecting `data:` JSON frames, until `until` matches or a deadline —
 * then cancel (the stream is keep-alive and never ends on its own). */
async function collectFrames(
  url: string,
  until: (frames: unknown[]) => boolean,
  deadlineMs = 3000
): Promise<unknown[]> {
  const res = await fetch(url);
  const reader = (res.body as ReadableStream<Uint8Array>).getReader();
  const decoder = new TextDecoder();
  const frames: unknown[] = [];
  let buffer = "";
  const start = Date.now();
  try {
    while (Date.now() - start < deadlineMs) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      const parts = buffer.split("\n\n");
      buffer = parts.pop() ?? "";
      for (const part of parts) {
        const line = part.split("\n").find((l) => l.startsWith("data: "));
        if (line) frames.push(JSON.parse(line.slice(6)));
      }
      if (until(frames)) break;
    }
  } finally {
    await reader.cancel();
  }
  return frames;
}

const describeIfRust = rustEngineAvailable() ? describe : describe.skip;

describeIfRust("adriane dev — run inspector (ADR DX batch 4)", () => {
  const saved: Record<string, string | undefined> = {};
  beforeEach(() => {
    saved.ADRIANE_SDK_ENGINE = process.env.ADRIANE_SDK_ENGINE;
    process.env.ADRIANE_SDK_ENGINE = "rust";
  });
  afterEach(() => {
    if (saved.ADRIANE_SDK_ENGINE === undefined) delete process.env.ADRIANE_SDK_ENGINE;
    else process.env.ADRIANE_SDK_ENGINE = saved.ADRIANE_SDK_ENGINE;
  });

  it("serves the inspector page + streams a governed run's events and explain over SSE", async () => {
    const app = createGraph({ name: "inspected" })
      .node("write", async () => ({ draft: "hi" }))
      .humanGate("review")
      .node("publish", async () => ({ published: true }))
      .edge("write", "review")
      .edge("review", "publish")
      .compile();

    const inspector = await serveInspector(app, {}, { port: 0 });
    try {
      await inspector.done; // run drives to the human-gate suspension

      // The page is served and self-contained.
      const html = await (await fetch(inspector.url)).text();
      expect(html).toContain("Adriane");
      expect(html).toContain("/events");

      // The SSE feed replays the run: node events + a suspended `explain` frame.
      const frames = (await collectFrames(
        `${inspector.url}events`,
        (fs) => fs.some((f) => (f as { kind: string }).kind === "run")
      )) as Array<{ kind: string; event?: { type: string }; explanation?: { status?: string } }>;

      const types = frames.filter((f) => f.kind === "event").map((f) => f.event?.type);
      expect(types).toContain("node_started");
      expect(types).toContain("node_completed");
      const explain = frames.find((f) => f.kind === "explain");
      expect(explain?.explanation?.status).toBe("suspended");
      const run = frames.find((f) => f.kind === "run") as { status: string } | undefined;
      expect(run?.status).toBe("suspended");
    } finally {
      await inspector.close();
    }
  });

  it("only the inspector page itself can resume the run, and only on its own host", async () => {
    const app = createGraph({ name: "inspected-guarded" })
      .node("write", async () => ({ draft: "<img src=x onerror=alert(1)>" }))
      .humanGate("review")
      .node("publish", async () => ({ published: true }))
      .edge("write", "review")
      .edge("review", "publish")
      .compile();

    const inspector = await serveInspector(app, {}, { port: 0 });
    const status = (path: string, init: { method?: string; host?: string; token?: string }) =>
      new Promise<number>((resolve, reject) => {
        const target = new URL(path, inspector.url);
        const req = httpRequest(
          {
            host: target.hostname,
            port: target.port,
            path: target.pathname,
            method: init.method ?? "GET",
            headers: {
              host: init.host ?? target.host,
              ...(init.token === undefined ? {} : { "x-inspector-token": init.token })
            }
          },
          (res) => {
            res.resume();
            resolve(res.statusCode ?? 0);
          }
        );
        req.on("error", reject);
        req.end();
      });
    try {
      await inspector.done;
      const html = await (await fetch(inspector.url)).text();
      // Run data is rendered as text only.
      expect(html).not.toContain("innerHTML");
      expect(html).not.toContain("insertAdjacentHTML");
      const token = /name="inspector-token" content="([0-9a-f]+)"/.exec(html)?.[1];
      expect(token).toBeDefined();

      // A forged cross-site POST (no token / wrong token) cannot cross the gate.
      expect(await status("/resume", { method: "POST" })).toBe(403);
      expect(await status("/resume", { method: "POST", token: "0".repeat(48) })).toBe(403);
      // A rebinding hostname is refused outright.
      expect(await status("/", { host: `attacker.example:${new URL(inspector.url).port}` })).toBe(403);
      expect(await status("/events", { host: "attacker.example" })).toBe(403);

      expect(await status("/resume", { method: "POST", token })).toBe(202);
    } finally {
      await inspector.close();
    }
  });
});
