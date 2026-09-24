import type { Loc } from "../ast/types.js";

export const createLoc = (file: string, line = 1, col = 1): Loc => ({
  line,
  col,
  file
});
