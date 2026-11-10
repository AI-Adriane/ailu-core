import { expect, expectTypeOf, it } from "vitest";
import {
  createGraph,
  InvalidFanOutLimitsError,
  type CompileGraphOptions,
  type FanOutLimits,
  type GraphCompileError,
  type Result,
  type TextModelAccessSnapshot
} from "./index.js";

/**
 * `compile` and `safeCompile` keep the types published in 2.8.5; ADR 0055 only adds two optional
 * fields to their options. `tsc` is the test: a wrong type fails `typecheck`. The function is
 * never called, so no graph is compiled.
 */
const contract = (snapshot: TextModelAccessSnapshot): void => {
  const builder = createGraph({ name: "types" });
  type Compiled = ReturnType<typeof builder.compile>;
  type CompileOnly = Result<Compiled, GraphCompileError>;
  type WithLimits = Result<Compiled, GraphCompileError | InvalidFanOutLimitsError>;

  // Every call valid in 2.8.5 keeps exactly its type.
  const limits: { fanOutLimits?: FanOutLimits } = {};
  expectTypeOf(builder.safeCompile()).toEqualTypeOf<CompileOnly>();
  expectTypeOf(builder.safeCompile({})).toEqualTypeOf<WithLimits>();
  expectTypeOf(builder.safeCompile({ fanOutLimits: undefined })).toEqualTypeOf<WithLimits>();
  expectTypeOf(builder.safeCompile({ fanOutLimits: { maxItems: 1 } })).toEqualTypeOf<WithLimits>();
  expectTypeOf(builder.safeCompile(limits)).toEqualTypeOf<WithLimits>();
  expectTypeOf(builder.compile()).toEqualTypeOf<Compiled>();
  expectTypeOf(builder.compile(limits)).toEqualTypeOf<Compiled>();
  // As in 2.8.5, an options argument is required once one is passed.
  // @ts-expect-error -- `undefined` is not a CompileGraphOptions
  builder.safeCompile(undefined);

  // A variable inferred from one call takes the result of another, as in 2.8.5.
  let reused = builder.safeCompile({});
  reused = builder.safeCompile({ fanOutLimits: { maxItems: 1 } });
  void reused;

  // The two new fields are accepted, alone or next to the limits, and widen nothing else.
  const options: CompileGraphOptions = { tenantId: "tenant-a", textModelAccess: snapshot };
  expectTypeOf(builder.safeCompile(options)).toEqualTypeOf<WithLimits>();
  expectTypeOf(builder.safeCompile({ tenantId: "tenant-a" })).toEqualTypeOf<WithLimits>();
  expectTypeOf(
    builder.safeCompile({ tenantId: "tenant-a", fanOutLimits: { maxItems: 1 } })
  ).toEqualTypeOf<WithLimits>();
  expectTypeOf(builder.compile(options)).toEqualTypeOf<Compiled>();
  expectTypeOf<CompileGraphOptions>().toEqualTypeOf<{
    fanOutLimits?: FanOutLimits;
    tenantId?: string;
    textModelAccess?: TextModelAccessSnapshot;
  }>();

  // The error of a call without limits is handled as in 2.8.5: narrow, then read.
  const result = builder.safeCompile({ tenantId: "tenant-a" });
  if (!result.success) {
    // @ts-expect-error -- the union has no `errors` until it is narrowed
    void result.error.errors;
    if (result.error instanceof InvalidFanOutLimitsError) {
      expectTypeOf(result.error).toEqualTypeOf<InvalidFanOutLimitsError>();
    } else {
      expectTypeOf(result.error).toEqualTypeOf<GraphCompileError>();
      expectTypeOf(result.error.errors).toBeArray();
    }
  }
};

it("compile options keep the 2.8.5 types and take the admission fields (checked by tsc)", () => {
  expect(contract).toBeTypeOf("function");
});
