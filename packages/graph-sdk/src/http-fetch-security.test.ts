import { lookup } from "node:dns";
import type { LookupAddress } from "node:dns";
import { afterEach, describe, expect, it, vi } from "vitest";
import { MockAgent } from "undici";

import { components, type HttpFetchParams, type HttpFetchRequestInit, type HttpFetchResponseLike } from "./components.js";
import * as policy from "./http-fetch-policy.js";

vi.mock("node:dns", () => ({ lookup: vi.fn() }));

const response = (): HttpFetchResponseLike => ({
  status: 200, ok: true, text: () => '{"hello":"world"}',
  headers: { get: () => "application/json" }
});

const invoke = (params: HttpFetchParams, channels: Record<string, unknown> = {}) => {
  const handler = components.httpFetch(params);
  return handler(undefined, { channels } as Parameters<typeof handler>[1], {} as Parameters<typeof handler>[2]);
};

const resolveTo = (addresses: LookupAddress[]) => {
  vi.mocked(lookup).mockImplementation(((_host: string, _options: unknown, callback: (err: null, records: LookupAddress[]) => void) => {
    callback(null, addresses);
  }) as typeof lookup);
};

afterEach(() => { vi.restoreAllMocks(); vi.mocked(lookup).mockReset(); });

describe("httpFetch destination boundary", () => {
  it("blocks channel-controlled metadata on the ambient fetch path", async () => {
    const ambient = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("secret"));
    expect(await invoke({ urlFrom: "url", allowedOrigins: ["https://169.254.169.254"], into: "result" }, { url: "https://169.254.169.254/latest/meta-data/" }))
      .toMatchObject({ result: { ok: false } });
    expect(ambient).not.toHaveBeenCalled();
  });

  it.each([
    "http://127.0.0.1/", "http://127.1/", "http://2130706433/", "http://0x7f000001/",
    "http://0177.0.0.1/", "http://0.0.0.0/", "http://10.0.0.1/", "http://172.16.0.1/",
    "http://192.168.1.1/", "http://169.254.169.254/latest/meta-data/", "http://100.100.100.200/",
    "http://[::1]/", "http://[::]/", "http://[fc00::1]/", "http://[fe80::1]/",
    "http://[::ffff:127.0.0.1]/", "http://[64:ff9b::a00:1]/", "http://[2002:7f00:1::]/",
    "http://localhost/", "http://service.localhost./", "http://example.com:8080/",
    "file:///etc/passwd", "ftp://example.com/file", "https://user:pass@example.com/",
    "data:text/plain,hello", "//example.com/path", "not a URL"
  ])("denies %s before any transport is invoked", async (url) => {
    const fetchImpl = vi.fn(response);
    const ambient = vi.spyOn(globalThis, "fetch");
    expect(await invoke({ url, into: "result", fetchImpl })).toMatchObject({ result: { ok: false, error: expect.any(String) } });
    expect(fetchImpl).not.toHaveBeenCalled();
    expect(ambient).not.toHaveBeenCalled();
  });

  it("denies channel URLs without host authorization, including literal fallback", async () => {
    const fetchImpl = vi.fn(response);
    for (const channels of [{ url: "https://example.com/" }, { url: "" }, {}]) {
      expect(await invoke({ urlFrom: "url", url: "https://example.com/", into: "result", fetchImpl }, channels))
        .toMatchObject({ result: { ok: false } });
    }
    expect(fetchImpl).not.toHaveBeenCalled();
  });

  it.each([
    "https://example.com.evil.test/", "https://evil.test/?url=https://example.com",
    "https://example.com@evil.test/", "http://example.com/", "https://example.com:444/",
    "https://example.com./", "https://127.0.0.1/", "https://[::ffff:a00:1]/"
  ])("does not authorize channel origin bypass %s", async (url) => {
    const fetchImpl = vi.fn(response);
    expect(await invoke({ urlFrom: "url", url: "https://example.com/", allowedOrigins: ["https://example.com"], into: "result", fetchImpl }, { url }))
      .toMatchObject({ result: { ok: false } });
    expect(fetchImpl).not.toHaveBeenCalled();
  });

  it("preserves authorized POST, headers, body, JSON and immutable host origins", async () => {
    const fetchImpl = vi.fn(response);
    const origins = ["https://example.com"];
    const handler = components.httpFetch({ urlFrom: "url", allowedOrigins: origins, into: "result", method: "POST", headers: { authorization: "Bearer test" }, body: "payload", timeoutMs: 1000, fetchImpl });
    origins.push("https://evil.test");
    const run = (url: string) => handler(undefined, { channels: { url } } as unknown as Parameters<typeof handler>[1], {} as Parameters<typeof handler>[2]);
    expect(await run("https://EXAMPLE.com:443/path?q=1")).toEqual({ result: { status: 200, ok: true, body: '{"hello":"world"}', json: { hello: "world" } } });
    expect(fetchImpl).toHaveBeenCalledWith("https://example.com/path?q=1", expect.objectContaining({ method: "POST", headers: { authorization: "Bearer test" }, body: "payload", signal: expect.any(AbortSignal), redirect: "error", dispatcher: expect.any(Object) }));
    expect(await run("https://evil.test/")).toMatchObject({ result: { ok: false } });
    expect(fetchImpl).toHaveBeenCalledTimes(1);
  });

  it.each(["Host", "HOST", ":authority"])("rejects %s authority overrides", async (header) => {
    const fetchImpl = vi.fn(response);
    expect(await invoke({ url: "https://example.com/", headers: { [header]: "internal" }, into: "result", fetchImpl })).toMatchObject({ result: { ok: false } });
    expect(fetchImpl).not.toHaveBeenCalled();
  });

  it("rejects a redirect before reading its body", async () => {
    const text = vi.fn(() => "secret");
    expect(await invoke({ url: "https://example.com/", into: "result", fetchImpl: () => ({ ...response(), status: 302, text }) })).toMatchObject({ result: { ok: false, error: expect.stringContaining("redirect") } });
    expect(text).not.toHaveBeenCalled();
  });

  it("preserves graceful timeout failures for an authorized request", async () => {
    const fetchImpl = vi.fn((_url: string, init: HttpFetchRequestInit) => new Promise<HttpFetchResponseLike>((_resolve, reject) => {
      init.signal?.addEventListener("abort", () => reject(new Error("request aborted")), { once: true });
    }));
    expect(await invoke({ url: "https://example.com/", into: "result", timeoutMs: 5, fetchImpl }))
      .toEqual({ result: { ok: false, error: "request aborted" } });
    expect(fetchImpl).toHaveBeenCalledTimes(1);
  });

  it("the real default fetch does not follow a public-to-metadata redirect", async () => {
    const agent = new MockAgent();
    agent.disableNetConnect();
    vi.spyOn(agent, "destroy").mockImplementation(() => agent.close());
    agent.get("https://example.com").intercept({ path: "/" }).reply(302, "", { headers: { location: "http://169.254.169.254/latest/meta-data/" } });
    const internal = vi.fn(() => ({ statusCode: 200, data: "secret" }));
    agent.get("http://169.254.169.254").intercept({ path: "/latest/meta-data/" }).reply(internal);
    vi.spyOn(policy, "createHttpFetchDispatcher").mockReturnValue(agent);
    expect(await invoke({ url: "https://example.com/", into: "result" })).toMatchObject({ result: { ok: false } });
    expect(internal).not.toHaveBeenCalled();
  });

  it("keeps the real fetch response and non-2xx contract", async () => {
    const agent = new MockAgent();
    agent.disableNetConnect();
    vi.spyOn(agent, "destroy").mockImplementation(() => agent.close());
    agent.get("https://example.com").intercept({ path: "/missing" }).reply(404, "not found");
    vi.spyOn(policy, "createHttpFetchDispatcher").mockReturnValue(agent);
    expect(await invoke({ url: "https://example.com/missing", into: "result" })).toEqual({ result: { status: 404, ok: false, body: "not found" } });
  });

  it("default fetch rejects private DNS answers before a connection", async () => {
    resolveTo([{ address: "127.0.0.1", family: 4 }]);
    expect(await invoke({ urlFrom: "url", allowedOrigins: ["https://example.com"], into: "result" }, { url: "https://example.com/" }))
      .toMatchObject({ result: { ok: false } });
    expect(lookup).toHaveBeenCalledWith("example.com", { all: true, verbatim: true }, expect.any(Function));
  });
});

describe("connection-time DNS policy", () => {
  it.each([
    "127.0.0.1", "10.1.2.3", "172.31.255.255", "192.168.1.1", "169.254.169.254", "100.64.0.1",
    "0.0.0.0", "192.0.0.1", "198.18.0.1", "224.0.0.1", "255.255.255.255", "::1", "fe80::1",
    "fc00::1", "::ffff:127.0.0.1", "64:ff9b::a9fe:a9fe", "2002:a00:1::", "2001:db8::1"
  ])("denies every answer set containing %s", (address) => {
    resolveTo([{ address: "93.184.216.34", family: 4 }, { address, family: address.includes(":") ? 6 : 4 }]);
    const callback = vi.fn();
    policy.publicHttpLookup("example.com", { all: true }, callback);
    expect(callback).toHaveBeenCalledWith(expect.any(Error), "");
  });

  it("passes exact public DNS results to the socket and revalidates every lookup", () => {
    const records = [{ address: "93.184.216.34", family: 4 }, { address: "2606:4700:4700::1111", family: 6 }];
    resolveTo(records);
    const callback = vi.fn();
    policy.publicHttpLookup("example.com", { all: true }, callback);
    expect(callback).toHaveBeenCalledWith(null, records);
    expect(lookup).toHaveBeenCalledTimes(1);
    resolveTo([{ address: "169.254.169.254", family: 4 }]);
    policy.publicHttpLookup("example.com", { all: true }, callback);
    expect(callback).toHaveBeenLastCalledWith(expect.any(Error), "");
    expect(lookup).toHaveBeenCalledTimes(2);
  });

  it("handles single-address family selection and empty answers fail closed", () => {
    resolveTo([{ address: "2606:4700:4700::1111", family: 6 }, { address: "93.184.216.34", family: 4 }]);
    const callback = vi.fn();
    policy.publicHttpLookup("example.com", { family: 4 }, callback);
    expect(callback).toHaveBeenCalledWith(null, "93.184.216.34", 4);
    resolveTo([]);
    policy.publicHttpLookup("example.com", {}, callback);
    expect(callback).toHaveBeenLastCalledWith(expect.any(Error), "");
  });
});
