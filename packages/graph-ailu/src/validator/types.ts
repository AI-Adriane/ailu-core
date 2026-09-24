import type { Loc } from "../ast/types";

export type Diagnostic = {
  code: string;
  message: string;
  loc: Loc;
  severity: "error" | "warning";
};
