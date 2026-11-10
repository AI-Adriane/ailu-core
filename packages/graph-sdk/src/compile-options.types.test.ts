import { expect, expectTypeOf, it } from "vitest";
import {
  createGraph,
  type CompileGraphOptions,
  type FanOutLimits,
  type GraphCompileError,
  type InvalidFanOutLimitsError,
  type Result,
  type TextModelAccessSnapshot
} from "./index.js";

/**
 * The types of `compile` and `safeCompile`, as two contracts read them: 2.8.5 (`fanOutLimits`)
 * and the admission options of ADR 0055. `tsc` is the test: a wrong return type fails
 * `typecheck`. The function is never called, so no graph is compiled.
 */
const contracts = (snapshot: TextModelAccessSnapshot): void => {
  const builder = createGraph({ name: "types" });
  type Compiled = ReturnType<typeof builder.compile>;
  type CompileOnly = Result<Compiled, GraphCompileError>;
  type WithLimits = Result<Compiled, GraphCompileError | InvalidFanOutLimitsError>;

  // 2.8.5: no argument reports compile errors only; fan-out limits add their own error.
  const limits: { fanOutLimits?: FanOutLimits } = {};
  const options: CompileGraphOptions = {};
  expectTypeOf(builder.safeCompile()).toEqualTypeOf<CompileOnly>();
  expectTypeOf(builder.safeCompile({ fanOutLimits: { maxItems: 1 } })).toEqualTypeOf<WithLimits>();
  expectTypeOf(builder.safeCompile(limits)).toEqualTypeOf<WithLimits>();
  expectTypeOf(builder.safeCompile(options)).toEqualTypeOf<WithLimits>();
  expectTypeOf(builder.compile()).toEqualTypeOf<Compiled>();
  expectTypeOf(builder.compile(limits)).toEqualTypeOf<Compiled>();

  // ADR 0055: admission options alone report compile errors only.
  const admission: { tenantId?: string; textModelAccess?: TextModelAccessSnapshot } = {};
  expectTypeOf(builder.safeCompile({ tenantId: "tenant-a" })).toEqualTypeOf<CompileOnly>();
  expectTypeOf(
    builder.safeCompile({ tenantId: "tenant-a", textModelAccess: snapshot })
  ).toEqualTypeOf<CompileOnly>();
  expectTypeOf(builder.safeCompile(admission)).toEqualTypeOf<CompileOnly>();
  expectTypeOf(builder.safeCompile(undefined)).toEqualTypeOf<CompileOnly>();
  expectTypeOf(builder.compile({ tenantId: "tenant-a" })).toEqualTypeOf<Compiled>();
  const refused = builder.safeCompile({ tenantId: "tenant-a" });
  if (!refused.success) expectTypeOf(refused.error.errors).toBeArray();

  // Both together keep the fan-out error.
  expectTypeOf(
    builder.safeCompile({ tenantId: "tenant-a", fanOutLimits: { maxItems: 1 } })
  ).toEqualTypeOf<WithLimits>();

  // The one call the two contracts type differently: an empty object reads as ADR 0055
  // (compile errors only), which still fits where 2.8.5 expected the wider type.
  expectTypeOf(builder.safeCompile({})).toEqualTypeOf<CompileOnly>();
  expectTypeOf(builder.safeCompile({})).toMatchTypeOf<WithLimits>();
};

it("compile options keep the 2.8.5 and the ADR 0055 types (checked by tsc)", () => {
  expect(contracts).toBeTypeOf("function");
});
