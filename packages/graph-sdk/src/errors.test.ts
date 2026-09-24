import { describe, expect, it } from "vitest";

import {
  MissingProviderKeyError,
  NoProviderInEnvError,
  UnknownProviderError
} from "@ailu/model-core";

import { RustEngineRequiredError } from "./compiled-graph.js";
import {
  AiluSdkError,
  DuplicateNodeError,
  GovernanceMiddlewareRejectedError,
  ResumeStateNotFoundError,
  UnknownNodeError
} from "./errors.js";

/** ADR errors-that-teach: every error carries a stable code + an actionable hint + a doc link,
 * the message stays what was thrown, and `.format()` renders all three. */
describe("errors that teach", () => {
  it("SDK errors carry code/hint/docUrl and a stable message", () => {
    const dup = new DuplicateNodeError("greet");
    expect(dup).toBeInstanceOf(AiluSdkError);
    expect(dup.code).toBe("ADR_DUPLICATE_NODE");
    expect(dup.hint).toContain("greet"); // the hint names the offending id
    expect(dup.docUrl).toContain("#adr_duplicate_node");
    // message unchanged (existing assertions hold):
    expect(dup.message).toBe("A node with id 'greet' was already added to this graph.");
  });

  it("format() renders message + hint + docs", () => {
    const out = new GovernanceMiddlewareRejectedError("approvalGate").format();
    expect(out).toContain("approvalGate");
    expect(out).toMatch(/→ /); // the hint line
    expect(out).toContain("docs: https://");
  });

  it("RustEngineRequiredError + ResumeStateNotFoundError are typed + coded", () => {
    expect(new RustEngineRequiredError("auto").code).toBe("ADR_RUST_ENGINE_REQUIRED");
    expect(new RustEngineRequiredError("auto")).toBeInstanceOf(AiluSdkError);
    const resume = new ResumeStateNotFoundError("run-7");
    expect(resume.code).toBe("ADR_NO_SUSPENDED_STATE");
    expect(resume.message).toContain("run-7");
  });

  it("UnknownNodeError hint teaches the fix", () => {
    const err = new UnknownNodeError("publish", "edge('write' → 'publish')");
    expect(err.code).toBe("ADR_UNKNOWN_NODE");
    expect(err.hint).toContain('node("publish"');
  });

  it("model-core provider errors carry codes + hints", () => {
    expect(new UnknownProviderError("cohere", ["openai", "anthropic"]).code).toBe(
      "ADR_UNKNOWN_PROVIDER"
    );
    const missing = new MissingProviderKeyError("openai", "OPENAI_API_KEY");
    expect(missing.code).toBe("ADR_MISSING_PROVIDER_KEY");
    expect(missing.hint).toContain("OPENAI_API_KEY");
    expect(new NoProviderInEnvError(["OPENAI_API_KEY"]).code).toBe("ADR_NO_PROVIDER_IN_ENV");
  });
});
