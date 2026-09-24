import { readFile } from "node:fs/promises";

export const publishCommand = async (file: string, registryUrl: string): Promise<number> => {
  const content = await readFile(file, "utf8");
  const response = await fetch(registryUrl, {
    method: "POST",
    headers: { "content-type": "application/yaml" },
    body: content
  });
  if (!response.ok) {
    process.stderr.write(`Publish failed: ${response.status}\n`);
    return 1;
  }
  process.stdout.write("Published successfully.\n");
  return 0;
};
