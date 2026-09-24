import { defineConfig } from "tsup";

// The CLI reaches into the sibling engine packages by relative source path
// (e.g. ../../graph-runtime/src/index.js), so tsup/esbuild inlines the whole
// engine into ONE self-contained executable — nothing else needs publishing.
// Only `commander` stays external (declared in package.json `dependencies`).
// The `#!/usr/bin/env node` shebang in bin/ailu.ts is preserved by tsup, so
// the emitted dist/ailu.js is directly runnable as the `ailu` bin.
export default defineConfig({
  entry: { ailu: "bin/ailu.ts" },
  format: ["esm"],
  target: "es2022",
  platform: "node",
  // A CLI ships an executable, not a typed library surface — skip declarations.
  dts: false,
  sourcemap: true,
  clean: true,
  treeshake: true,
  splitting: false,
  external: ["commander"]
});
