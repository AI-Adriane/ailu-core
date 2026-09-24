import { Command } from "commander";

import { compileCommand } from "./commands/compile.js";
import { diffCommand } from "./commands/diff.js";
import { initCommand } from "./commands/init.js";
import { publishCommand } from "./commands/publish.js";
import { runCommand } from "./commands/run.js";
import { validateCommand } from "./commands/validate.js";

export const createCli = (): Command => {
  const program = new Command();
  program.name("ailu").description("Ailu CLI");

  program
    .command("validate")
    .argument("<file>")
    .action(async (file: string) => {
      process.exitCode = await validateCommand(file);
    });

  program
    .command("compile")
    .argument("<file>")
    .requiredOption("--out <dir>")
    .action(async (file: string, opts: { out: string }) => {
      process.exitCode = await compileCommand(file, opts.out);
    });

  program
    .command("run")
    .argument("<file>")
    .option("--input <json>")
    .option("--watch")
    .action(async (file: string, opts: { input?: string; watch?: boolean }) => {
      process.exitCode = await runCommand(file, opts.input, Boolean(opts.watch));
    });

  program
    .command("publish")
    .argument("<file>")
    .requiredOption("--registry <url>")
    .action(async (file: string, opts: { registry: string }) => {
      process.exitCode = await publishCommand(file, opts.registry);
    });

  program
    .command("diff")
    .argument("<left>")
    .argument("<right>")
    .action(async (left: string, right: string) => {
      process.exitCode = await diffCommand(left, right);
    });

  program
    .command("init")
    .argument("<kind>")
    .requiredOption("--id <id>")
    .requiredOption("--out <file>")
    .action(async (kind: "graph" | "agent" | "prompt", opts: { id: string; out: string }) => {
      process.exitCode = await initCommand(kind, opts.id, opts.out);
    });

  return program;
};

export const runCli = async (argv = process.argv): Promise<void> => {
  await createCli().parseAsync(argv);
};
