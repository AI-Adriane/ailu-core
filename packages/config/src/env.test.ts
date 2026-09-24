import { describe, expect, it } from "vitest";

import { ConfigValidationError } from "./errors.js";
import { parseEnv } from "./env.js";

describe("parseEnv", () => {
  it("parses a valid environment", () => {
    const env = parseEnv({
      NODE_ENV: "local",
      PORT: "4000",
      DATABASE_URL: "postgres://localhost:5432/ailu",
      REDIS_URL: "redis://localhost:6379",
      JWT_SECRET: "super-secret",
      JWT_EXPIRY: "2h",
      LOG_LEVEL: "debug"
    });

    expect(env.PORT).toBe(4000);
    expect(env.NODE_ENV).toBe("local");
    expect(env.LOG_LEVEL).toBe("debug");
  });

  it("throws when required variable is missing", () => {
    expect(() =>
      parseEnv({
        NODE_ENV: "local",
        REDIS_URL: "redis://localhost:6379",
        JWT_SECRET: "super-secret",
        LOG_LEVEL: "info"
      })
    ).toThrowError(ConfigValidationError);
  });

  it("applies default values", () => {
    const env = parseEnv({
      NODE_ENV: "staging",
      DATABASE_URL: "postgres://localhost:5432/ailu",
      REDIS_URL: "redis://localhost:6379",
      JWT_SECRET: "super-secret",
      // Required outside NODE_ENV=local by the fail-secure superRefine.
      WORKER_TOKEN: "staging-worker-secret"
    });

    expect(env.PORT).toBe(3000);
    expect(env.JWT_EXPIRY).toBe("1h");
    expect(env.LOG_LEVEL).toBe("info");
  });

  it("allows an absent WORKER_TOKEN in NODE_ENV=local", () => {
    const env = parseEnv({
      NODE_ENV: "local",
      DATABASE_URL: "postgres://localhost:5432/ailu",
      REDIS_URL: "redis://localhost:6379",
      JWT_SECRET: "super-secret"
    });

    expect(env.WORKER_TOKEN).toBeUndefined();
  });

  it("fails secure: rejects a missing WORKER_TOKEN outside NODE_ENV=local", () => {
    expect(() =>
      parseEnv({
        NODE_ENV: "production",
        DATABASE_URL: "postgres://localhost:5432/ailu",
        REDIS_URL: "redis://localhost:6379",
        JWT_SECRET: "super-secret"
      })
    ).toThrowError(ConfigValidationError);
  });

  it("accepts a WORKER_TOKEN outside NODE_ENV=local", () => {
    const env = parseEnv({
      NODE_ENV: "production",
      DATABASE_URL: "postgres://localhost:5432/ailu",
      REDIS_URL: "redis://localhost:6379",
      JWT_SECRET: "super-secret",
      WORKER_TOKEN: "prod-worker-secret"
    });

    expect(env.WORKER_TOKEN).toBe("prod-worker-secret");
  });
});
