import type { ZodIssue } from "zod/v3";

export class ConfigValidationError extends Error {
  public readonly issues: ZodIssue[];

  public constructor(issues: ZodIssue[]) {
    super("Invalid environment configuration.");
    this.name = "ConfigValidationError";
    this.issues = issues;
  }
}
