import { defineConfig } from "tsup";

export default defineConfig({
  entry: { cli: "src/cli.ts", index: "src/index.ts" },
  format: ["esm"],
  // .d.ts emit is skipped: this ships primarily as the `ailu-verify` CLI. `tsc -p` still
  // typechecks the source; the verifyCapsule library API is re-exported from index for JS consumers.
  dts: false,
  clean: true,
  // graph-sdk (+ its napi addon) stays external — installed alongside via npm, not inlined.
  external: ["@ailu-ai/graph-sdk"]
});
