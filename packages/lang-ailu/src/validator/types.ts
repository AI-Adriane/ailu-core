import type { Loc } from "../ast/types.js";

export type Diagnostic = {
  code: string;
  message: string;
  loc: Loc;
  severity: "error" | "warning";
};
