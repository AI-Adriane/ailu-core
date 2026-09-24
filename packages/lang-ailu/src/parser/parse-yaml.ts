import { load } from "js-yaml";

export const parseYaml = (content: string, file: string): unknown => {
  try {
    return load(content);
  } catch (error) {
    const message = error instanceof Error ? error.message : "Unknown YAML parse error.";
    throw new Error(`Invalid YAML in ${file}: ${message}`);
  }
};
