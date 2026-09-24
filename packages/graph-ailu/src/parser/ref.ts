import type { VersionedRef } from "../ast/types";

const REF_REGEX = /^([^@]+)@(\d+\.\d+\.\d+)$/;

export const parseVersionedRef = (value: string): VersionedRef | undefined => {
  const match = REF_REGEX.exec(value.trim());
  if (match === null) {
    return undefined;
  }
  const id = match[1];
  const version = match[2];
  if (id === undefined || version === undefined) {
    return undefined;
  }
  return { id, version };
};

export const isValidSemver = (version: string): boolean => /^\d+\.\d+\.\d+$/.test(version);
