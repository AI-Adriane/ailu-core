import { randomBytes, timingSafeEqual } from "node:crypto";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";

import type { RunEvent } from "@ailu/graph-runtime";

import type { CompiledGraph } from "./compiled-graph.js";
import type { ChannelValues, InitialData } from "./typed.js";

/**
 * `ailu dev` — the local run inspector (ADR DX batch 4). Run a graph and **watch it think** in
 * the browser: a live node-by-node timeline, the lifecycle-event stream, each node's output, and a
 * **governance lens** that marks exactly where the run suspended (human gate / approval) and why —
 * with `explain()` and a one-click resume. Dependency-free (node:http only) and self-contained
 * (the page is inline HTML/JS — no CDN, no build step), so it drops into any project.
 *
 * v1 streams a single run live. The replay-from-checkpoint primitive now exists —
 * {@link replayCatalogGraph} forks a deterministic re-execution from any checkpoint (ADR 0038) and
 * {@link verifyReplayDecisions} proves it reproduces the attested decisions. Wiring that into a
 * one-click "rewind & replay" governance lens in this page is the remaining v2 surface.
 */
export type InspectorHandle = {
  /** The URL the inspector is served at. */
  url: string;
  /** A promise that resolves when the inspected run settles (completed / failed / suspended). */
  done: Promise<void>;
  /** Shut the server down. */
  close: () => Promise<void>;
};

export type InspectorOptions = {
  /** Port to listen on (default 4517; 0 picks a free port). */
  port?: number;
  /** Host to bind (default 127.0.0.1 — local only). */
  host?: string;
};

type Frame =
  | { kind: "event"; event: RunEvent }
  | { kind: "explain"; explanation: unknown }
  | { kind: "run"; status: string };

/** Serve a live inspector for a single run of `app`. Drives `app.run(initialData)`, streaming
 * every lifecycle event to the page over SSE; surfaces `explain()` on suspend/settle. */
export function serveInspector<TState extends ChannelValues>(
  app: CompiledGraph<TState>,
  initialData: InitialData<TState>,
  options: InspectorOptions = {}
): Promise<InspectorHandle> {
  const host = options.host ?? "127.0.0.1";
  const port = options.port ?? 4517;

  const frames: Frame[] = []; // replay buffer for late-joining clients
  const clients = new Set<ServerResponse>();
  let runId: string | undefined;
  // The resume action crosses a human gate, so only this page may trigger it: the page carries a
  // per-process token and sends it in a header. A cross-site page cannot read the token, and a
  // custom header forces a CORS preflight this server never answers.
  const resumeToken = randomBytes(24).toString("hex");
  let allowedHosts = new Set<string>();

  const push = (frame: Frame): void => {
    frames.push(frame);
    const line = `data: ${JSON.stringify(frame)}\n\n`;
    for (const res of clients) res.write(line);
  };

  const unsubscribe = app.onEvent((event) => {
    if (runId === undefined && "runId" in event) runId = String(event.runId);
    push({ kind: "event", event });
  });

  const server: Server = createServer((req: IncomingMessage, res: ServerResponse) => {
    // DNS rebinding: a hostile domain resolved to this address would otherwise be same-origin
    // with the inspector and could read the run's data and the resume token. Only the bound
    // address (and its loopback aliases) is served.
    if (!allowedHosts.has((req.headers.host ?? "").toLowerCase())) {
      res.writeHead(403).end();
      return;
    }
    const url = req.url ?? "/";
    if (url === "/" || url.startsWith("/?")) {
      res.writeHead(200, {
        "content-type": "text/html; charset=utf-8",
        "content-security-policy": "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'",
        "x-content-type-options": "nosniff"
      });
      res.end(INSPECTOR_HTML.replace("__RESUME_TOKEN__", resumeToken));
      return;
    }
    if (url === "/events") {
      res.writeHead(200, {
        "content-type": "text/event-stream",
        "cache-control": "no-cache",
        connection: "keep-alive"
      });
      for (const frame of frames) res.write(`data: ${JSON.stringify(frame)}\n\n`); // replay
      clients.add(res);
      req.on("close", () => clients.delete(res));
      return;
    }
    if (url.startsWith("/resume") && req.method === "POST") {
      const sent = Buffer.from(String(req.headers["x-inspector-token"] ?? ""));
      const expected = Buffer.from(resumeToken);
      if (sent.length !== expected.length || !timingSafeEqual(sent, expected)) {
        res.writeHead(403).end();
        return;
      }
      if (runId !== undefined) {
        void app.resume(runId as never).then(
          (state) => {
            push({ kind: "explain", explanation: app.explain(String(state.runId) as never) });
            push({ kind: "run", status: String(state.status) });
          },
          (err: unknown) => push({ kind: "run", status: `resume-error: ${String(err)}` })
        );
      }
      res.writeHead(202).end();
      return;
    }
    res.writeHead(404).end();
  });

  return new Promise<InspectorHandle>((resolve) => {
    server.listen(port, host, () => {
      const addr = server.address();
      const boundPort = typeof addr === "object" && addr !== null ? addr.port : port;
      const url = `http://${host}:${boundPort}/`;
      allowedHosts = new Set(
        [host, "localhost", "127.0.0.1", "[::1]"].map((name) => `${name}:${boundPort}`.toLowerCase())
      );

      // Drive the run; stream settles to the page.
      const done = app
        .run(initialData)
        .then((state) => {
          runId = String(state.runId);
          push({ kind: "explain", explanation: app.explain(runId as never) });
          push({ kind: "run", status: String(state.status) });
        })
        .catch((err: unknown) => {
          push({ kind: "run", status: `error: ${String(err)}` });
        });

      const close = async (): Promise<void> => {
        unsubscribe();
        for (const res of clients) res.end();
        clients.clear();
        await new Promise<void>((r) => server.close(() => r()));
      };

      resolve({ url, done, close });
    });
  });
}

/** The inline inspector page — a dependency-free timeline + event log + governance lens. */
const INSPECTOR_HTML = `<!doctype html><meta charset="utf-8"><meta name="inspector-token" content="__RESUME_TOKEN__"><title>Ailu dev — run inspector</title>
<style>
 :root{color-scheme:dark}
 body{margin:0;font:14px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace;background:#0b0d10;color:#d7dce2}
 header{padding:12px 16px;border-bottom:1px solid #1c2128;display:flex;gap:12px;align-items:center}
 header b{color:#fff;font-size:15px} #status{margin-left:auto;padding:2px 10px;border-radius:99px;background:#1c2128}
 #status.suspended{background:#3a2d00;color:#ffd479} #status.completed{background:#0f3a23;color:#7ee2a8}
 #status.failed{background:#3a1418;color:#ff8a8a}
 main{display:grid;grid-template-columns:1fr 1fr;gap:0;height:calc(100vh - 49px)}
 section{overflow:auto;padding:14px 16px} #timeline{border-right:1px solid #1c2128}
 h2{font-size:11px;text-transform:uppercase;letter-spacing:.08em;color:#6b7785;margin:0 0 10px}
 .node{padding:8px 10px;border:1px solid #1c2128;border-radius:8px;margin-bottom:8px}
 .node .id{color:#fff;font-weight:600} .node.done{border-color:#1f5e3a} .node.failed{border-color:#5e1f24}
 .node.gate{border-color:#5e4a1f;background:#161310}
 .node pre{margin:6px 0 0;color:#9aa6b2;white-space:pre-wrap;word-break:break-word;font-size:12.5px}
 .ev{color:#8b97a3;font-size:12.5px;padding:2px 0} .ev .t{color:#5b6b7b}
 #explain{margin-top:14px;padding:10px 12px;border:1px solid #2a2f37;border-radius:8px;background:#0f1216}
 #explain .s{color:#ffd479} button{font:inherit;background:#1f6feb;color:#fff;border:0;border-radius:7px;padding:6px 14px;cursor:pointer}
 button[disabled]{opacity:.4;cursor:default}
</style>
<header><b>Ailu</b> run inspector <span id="run"></span><span id="status">connecting…</span></header>
<main>
 <section id="timeline"><h2>Nodes</h2><div id="nodes"></div>
   <div id="explain" hidden><div class="s" id="esummary"></div><div id="enext"></div>
     <p><button id="resume" hidden>▶ Resume run</button></p></div>
 </section>
 <section id="log"><h2>Events</h2><div id="events"></div></section>
</main>
<script>
const nodes=document.getElementById("nodes"),events=document.getElementById("events"),
 statusEl=document.getElementById("status"),runEl=document.getElementById("run"),
 explainEl=document.getElementById("explain"),esum=document.getElementById("esummary"),
 enext=document.getElementById("enext"),resumeBtn=document.getElementById("resume");
const seen={};
// Node ids, outputs and errors are run data (possibly LLM- or user-controlled): always inserted
// as text, never as HTML.
function el(tag,cls,text){const d=document.createElement(tag);if(cls)d.className=cls;
 if(text!==undefined)d.textContent=String(text);return d;}
function node(id){if(seen[id])return seen[id];const d=el("div","node");
 d.appendChild(el("div","id",id));nodes.appendChild(d);seen[id]=d;return d;}
function log(t,id,error){const d=el("div","ev");d.appendChild(el("span","t",t));
 if(id){d.appendChild(document.createTextNode(" "));d.appendChild(el("b","",id));}
 if(error){d.appendChild(document.createTextNode(" — "+error));}
 events.appendChild(d);events.scrollTop=1e9;}
const es=new EventSource("/events");
es.onmessage=function(m){const f=JSON.parse(m.data);
 if(f.kind==="event"){const e=f.event,id=e.nodeId;
   if(e.type==="node_started"){node(id);}
   if(e.type==="node_completed"){const n=node(id);n.classList.add("done");
     if(e.output)n.appendChild(el("pre","",JSON.stringify(e.output,null,1)));}
   if(e.type==="node_failed"){node(id).classList.add("failed");}
   if(e.type==="run_suspended"){node(id).classList.add("gate");}
   log(e.type,id,e.error);}
 if(f.kind==="run"){statusEl.textContent=f.status;statusEl.className=f.status;}
 if(f.kind==="explain"){const x=f.explanation;explainEl.hidden=false;esum.textContent=x.summary||"";
   enext.textContent=x.suspended?("→ "+x.suspended.nextAction):"";
   if(x.status==="suspended"){resumeBtn.hidden=false;}else{resumeBtn.hidden=true;}}
};
resumeBtn.onclick=function(){resumeBtn.disabled=true;statusEl.textContent="resuming…";
 fetch("/resume",{method:"POST",headers:{"x-inspector-token":document.querySelector('meta[name="inspector-token"]').content}});};
</script>`;
