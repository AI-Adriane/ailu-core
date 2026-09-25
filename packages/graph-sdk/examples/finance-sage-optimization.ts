/**
 * Tutorial — Finance-ops optimization from a Sage accounting export (advanced).
 *
 * What you'll learn:
 *   - feeding a realistic dataset (a mock Sage journal export with planted issues) to an agent
 *     through plain tools: parsing, KPIs, anomaly detection, optimization proposals
 *   - the governance core: posting correcting entries is approval-gated. The run suspends, an
 *     ApprovalEngine records the request, the CFO approves, and the run resumes and posts the
 *     corrections exactly once
 *   - running a stored graph on the catalog path (`runCatalogGraph` / `resumeCatalogGraph`),
 *     with the tool code supplied per run
 *
 * Planted issues in the export: a duplicated supplier invoice, customer invoices paid 60+ days
 * late (high DSO), more than 50% manual entries, unmatched entries, one suspicious round
 * transfer, and one VAT line inconsistent with its base.
 *
 * The agent decides WHEN to post corrections; the tool decides WHAT is posted (the corrections
 * derived from the analysis); a human decides WHETHER. Self-verifying: every claim is checked
 * and the first failed check throws.
 *
 * Run it offline (the engine's deterministic mock calls each declared tool once):
 *   AILU_LLM_MOCK=1 pnpm --filter @ailu-ai/graph-sdk example:finance
 * With ANTHROPIC_API_KEY set, the analyst and the report writer run on Claude.
 */
import {
  createGraph,
  finalAnswer,
  InMemoryApprovalEngine,
  InMemoryToolRegistry,
  model,
  resumeCatalogGraph,
  runCatalogGraph,
  type AgentResult,
  type RunId,
  type ToolId
} from "@ailu-ai/graph-sdk";

// Self-check: fail loudly (throw) rather than print a wrong claim.
const check = (condition: boolean, label: string): void => {
  if (!condition) throw new Error(`Check failed: ${label}`);
  console.log(`  ✓ ${label}`);
};

// ── The mock Sage export (25 journal entries, issues planted on purpose) ─────
// Journal codes as Sage exports them: VEN = sales, ACH = purchases, BAN = bank. Accounts follow
// the French chart of accounts: 411* customers, 401* suppliers, 512 bank, 607 purchases,
// 44566 deductible VAT, 627 bank fees. `matchCode` links an invoice to its payment.
type JournalEntry = {
  journal: "VEN" | "ACH" | "BAN";
  date: string;
  account: string;
  label: string;
  debit: number;
  credit: number;
  matchCode: string | null;
  source: "manual" | "import";
  supplier?: string;
  customer?: string;
};

const SAGE_EXPORT: JournalEntry[] = [
  // VEN — customer invoices (receivables on 411*)
  { journal: "VEN", date: "2026-01-05", account: "411AUB", label: "Invoice F-C001 Maison Aubert", debit: 1200, credit: 0, matchCode: "AA", source: "import", customer: "Maison Aubert" },
  { journal: "VEN", date: "2026-01-12", account: "411BRU", label: "Invoice F-C002 Atelier Brun", debit: 980, credit: 0, matchCode: "AB", source: "manual", customer: "Atelier Brun" },
  { journal: "VEN", date: "2026-01-20", account: "411COS", label: "Invoice F-C003 Galerie Costa", debit: 2400, credit: 0, matchCode: "AC", source: "manual", customer: "Galerie Costa" },
  { journal: "VEN", date: "2026-02-03", account: "411ERR", label: "Invoice F-C004 Domaine Errel", debit: 1750, credit: 0, matchCode: "AD", source: "import", customer: "Domaine Errel" },
  { journal: "VEN", date: "2026-02-10", account: "411FAG", label: "Invoice F-C005 Librairie Fage", debit: 640, credit: 0, matchCode: null, source: "manual", customer: "Librairie Fage" },
  { journal: "VEN", date: "2026-02-18", account: "411AUB", label: "Invoice F-C006 Maison Aubert", debit: 3100, credit: 0, matchCode: null, source: "manual", customer: "Maison Aubert" },
  // BAN — bank: customer receipts (the match code links a receipt to its invoice)
  { journal: "BAN", date: "2026-01-25", account: "512", label: "Receipt F-C001", debit: 1200, credit: 0, matchCode: "AA", source: "import" },
  { journal: "BAN", date: "2026-03-20", account: "512", label: "Receipt F-C002", debit: 980, credit: 0, matchCode: "AB", source: "import" },
  { journal: "BAN", date: "2026-03-28", account: "512", label: "Receipt F-C003", debit: 2400, credit: 0, matchCode: "AC", source: "import" },
  { journal: "BAN", date: "2026-04-15", account: "512", label: "Receipt F-C004", debit: 1750, credit: 0, matchCode: "AD", source: "import" },
  { journal: "BAN", date: "2026-02-28", account: "512", label: "Internal transfer", debit: 10000, credit: 0, matchCode: null, source: "manual" },
  { journal: "BAN", date: "2026-03-31", account: "627", label: "Bank fees, March", debit: 0, credit: 38.5, matchCode: null, source: "import" },
  // ACH — supplier invoices (payables on 401*)
  { journal: "ACH", date: "2026-01-08", account: "401LUT", label: "Invoice Papeterie Lutece - office supplies", debit: 0, credit: 850, matchCode: "BA", source: "manual", supplier: "Papeterie Lutece" },
  { journal: "ACH", date: "2026-01-15", account: "401MAR", label: "Invoice Transports Marek - deliveries", debit: 0, credit: 1320, matchCode: "BB", source: "import", supplier: "Transports Marek" },
  { journal: "ACH", date: "2026-02-02", account: "401HEL", label: "Spring mock-up design services", debit: 0, credit: 2150, matchCode: null, source: "manual", supplier: "Studio Helio" },
  { journal: "ACH", date: "2026-02-09", account: "401HEL", label: "Spring mock-up design services", debit: 0, credit: 2150, matchCode: null, source: "manual", supplier: "Studio Helio" },
  { journal: "ACH", date: "2026-02-12", account: "607", label: "Display stands, net of VAT", debit: 1000, credit: 0, matchCode: null, source: "manual", supplier: "Mobilier Kova" },
  { journal: "ACH", date: "2026-02-12", account: "44566", label: "Deductible VAT on display stands (20%)", debit: 250, credit: 0, matchCode: null, source: "manual", supplier: "Mobilier Kova" },
  { journal: "ACH", date: "2026-02-12", account: "401KOV", label: "Invoice Mobilier Kova - display stands incl. VAT", debit: 0, credit: 1250, matchCode: null, source: "manual", supplier: "Mobilier Kova" },
  { journal: "ACH", date: "2026-02-20", account: "401ENE", label: "Invoice Energie Roule - electricity", debit: 0, credit: 410, matchCode: "BC", source: "import", supplier: "Energie Roule" },
  { journal: "ACH", date: "2026-03-01", account: "401IMP", label: "Invoice Imprimerie Sel - catalogs", debit: 0, credit: 760, matchCode: null, source: "manual", supplier: "Imprimerie Sel" },
  { journal: "ACH", date: "2026-03-05", account: "401NET", label: "Invoice Nettoyage Pur - March", debit: 0, credit: 290, matchCode: null, source: "manual", supplier: "Nettoyage Pur" },
  // BAN — supplier payments
  { journal: "BAN", date: "2026-01-30", account: "512", label: "Payment Papeterie Lutece", debit: 0, credit: 850, matchCode: "BA", source: "import" },
  { journal: "BAN", date: "2026-02-15", account: "512", label: "Payment Transports Marek", debit: 0, credit: 1320, matchCode: "BB", source: "manual" },
  { journal: "BAN", date: "2026-03-10", account: "512", label: "Payment Energie Roule", debit: 0, credit: 410, matchCode: "BC", source: "manual" }
];

// ── Pure analysis functions (the tools call these) ───────────────────────────
type Kpis = {
  avgDaysToCollect: number; // DSO: average days from customer invoice to receipt
  lateCollections: number; // customer invoices collected 60+ days after issue
  manualEntriesPct: number;
  matchedPct: number; // share of customer/supplier entries matched to a payment
  potentialDuplicates: number;
};

type Anomaly = { type: string; severity: "high" | "medium"; detail: string };

const round2 = (value: number): number => Math.round(value * 100) / 100;

const daysBetween = (from: string, to: string): number =>
  Math.round((new Date(to).getTime() - new Date(from).getTime()) / 86_400_000);

const eur = (amount: number): string =>
  new Intl.NumberFormat("en-US", { style: "currency", currency: "EUR" }).format(amount);

const isThirdParty = (entry: JournalEntry): boolean =>
  entry.account.startsWith("411") || entry.account.startsWith("401");

/** Days to collect each matched customer invoice (VEN/411*), paired with its bank receipt. */
const daysToCollect = (): number[] =>
  SAGE_EXPORT.filter((e) => e.journal === "VEN" && e.matchCode !== null).flatMap((invoice) => {
    const receipt = SAGE_EXPORT.find((e) => e.journal === "BAN" && e.matchCode === invoice.matchCode);
    return receipt === undefined ? [] : [daysBetween(invoice.date, receipt.date)];
  });

/** Supplier invoices posted more than once (same supplier, amount and label). */
const duplicateSupplierInvoices = (): Array<{ supplier: string; amount: number; label: string }> => {
  const groups = new Map<string, JournalEntry[]>();
  for (const entry of SAGE_EXPORT) {
    if (entry.journal === "ACH" && entry.credit > 0 && entry.supplier !== undefined) {
      const key = `${entry.supplier}|${entry.credit}|${entry.label}`;
      groups.set(key, [...(groups.get(key) ?? []), entry]);
    }
  }
  return [...groups.values()]
    .filter((group) => group.length > 1)
    .flatMap((group) => {
      const first = group[0];
      return first === undefined ? [] : [{ supplier: first.supplier ?? "?", amount: first.credit, label: first.label }];
    });
};

const computeKpis = (): Kpis => {
  const delays = daysToCollect();
  const thirdParty = SAGE_EXPORT.filter(isThirdParty);
  return {
    avgDaysToCollect: delays.length > 0 ? Math.round(delays.reduce((sum, days) => sum + days, 0) / delays.length) : 0,
    lateCollections: delays.filter((days) => days >= 60).length,
    manualEntriesPct: Math.round((SAGE_EXPORT.filter((e) => e.source === "manual").length / SAGE_EXPORT.length) * 100),
    matchedPct: Math.round((thirdParty.filter((e) => e.matchCode !== null).length / thirdParty.length) * 100),
    potentialDuplicates: duplicateSupplierInvoices().length
  };
};

const detectAnomalies = (): Anomaly[] => {
  const anomalies: Anomaly[] = [];

  for (const duplicate of duplicateSupplierInvoices()) {
    anomalies.push({
      type: "duplicate_supplier_invoice",
      severity: "high",
      detail: `"${duplicate.label}" from ${duplicate.supplier} posted twice (${eur(duplicate.amount)})`
    });
  }

  // A VAT line vs its base: same supplier + date; expected = base × 20%.
  for (const vat of SAGE_EXPORT.filter((e) => e.account.startsWith("44566"))) {
    const base = SAGE_EXPORT.find(
      (e) => e.account.startsWith("607") && e.supplier === vat.supplier && e.date === vat.date
    );
    if (base !== undefined && round2(vat.debit) !== round2(base.debit * 0.2)) {
      const expected = round2(base.debit * 0.2);
      anomalies.push({
        type: "vat_mismatch",
        severity: "high",
        detail:
          `Deductible VAT of ${eur(vat.debit)} on a base of ${eur(base.debit)} ` +
          `(expected ${eur(expected)} at 20%), a ${eur(vat.debit - expected)} gap`
      });
    }
  }

  for (const entry of SAGE_EXPORT) {
    const amount = entry.debit + entry.credit;
    if (entry.journal === "BAN" && entry.source === "manual" && amount >= 5000 && amount % 1000 === 0) {
      anomalies.push({
        type: "suspicious_round_amount",
        severity: "medium",
        detail: `"${entry.label}" of ${eur(amount)} entered manually and unmatched (${entry.date})`
      });
    }
  }

  const late = daysToCollect().filter((days) => days >= 60);
  if (late.length > 0) {
    anomalies.push({
      type: "late_payment",
      severity: "medium",
      detail: `${late.length} customer invoices collected 60 or more days after issue (high DSO)`
    });
  }

  const unmatched = SAGE_EXPORT.filter((e) => isThirdParty(e) && e.matchCode === null);
  if (unmatched.length > 0) {
    anomalies.push({
      type: "unmatched_entries",
      severity: "medium",
      detail: `${unmatched.length} customer/supplier entries not matched to a payment`
    });
  }

  return anomalies;
};

const proposeOptimizations = (kpis: Kpis, anomalies: Anomaly[]): string[] => {
  const recommendations: string[] = [];
  if (kpis.manualEntriesPct > 50) {
    recommendations.push(
      "Automate bookkeeping imports (bank feed + supplier invoice OCR) to cut manual entries " +
        `(${kpis.manualEntriesPct}% today).`
    );
  }
  if (kpis.matchedPct < 80) {
    recommendations.push(`Turn on automatic matching by invoice reference (matched today: ${kpis.matchedPct}%).`);
  }
  if (kpis.avgDaysToCollect > 45) {
    recommendations.push(
      `Automate customer reminders (day 15, 30, 45) to bring DSO down (${kpis.avgDaysToCollect} days on average).`
    );
  }
  if (kpis.potentialDuplicates > 0) {
    recommendations.push("De-duplicate supplier invoices at entry: check supplier + amount + label.");
  }
  if (anomalies.some((a) => a.type === "vat_mismatch")) {
    recommendations.push("Add automatic VAT checks (base × rate consistency) before validation.");
  }
  recommendations.push("Require human validation of manual entries above a threshold.");
  return recommendations;
};

// The correcting entries the analysis calls for (what the gated tool posts once approved).
const CORRECTIONS = [
  { account: "401HEL", label: "Reverse the duplicate Studio Helio invoice (posted twice)", amount: 2150 },
  { account: "44566", label: "Fix VAT on display stands: 250 → 200 (20% of a 1,000 base)", amount: -50 }
];

// ── Tools wired to the analysis functions ────────────────────────────────────
// Registered in call order: the analysis tools first, the gated posting tool last.
const toolsCalled = new Set<string>();
let postedBatches = 0;
let postedEntries = 0;
const passthrough = { parse: (value: unknown) => value };
const tools = new InMemoryToolRegistry();

const addTool = (
  name: string,
  description: string,
  run: () => unknown,
  options: { requiresApproval?: boolean; permissions?: string[] } = {}
): void => {
  tools.register(
    {
      id: name as ToolId,
      name,
      description,
      inputSchema: passthrough,
      outputSchema: passthrough,
      permissions: options.permissions ?? [],
      requiresApproval: options.requiresApproval ?? false,
      jsonSchema: { type: "object" } // these tools take no input
    },
    async () => {
      toolsCalled.add(name);
      return run();
    }
  );
};

addTool("parse_sage_export", "Parse the Sage export: line count and totals per journal.", () => ({
  totalLines: SAGE_EXPORT.length,
  journals: (["VEN", "ACH", "BAN"] as const).map((journal) => {
    const lines = SAGE_EXPORT.filter((e) => e.journal === journal);
    return {
      journal,
      lines: lines.length,
      totalDebit: round2(lines.reduce((sum, e) => sum + e.debit, 0)),
      totalCredit: round2(lines.reduce((sum, e) => sum + e.credit, 0))
    };
  })
}));
addTool("compute_kpis", "Compute KPIs: DSO, share of manual entries, matching rate, duplicates.", computeKpis);
addTool("detect_anomalies", "Detect anomalies: duplicates, VAT mismatches, round amounts, late payments.", () => ({
  anomalies: detectAnomalies()
}));
addTool("propose_optimizations", "Derive optimization recommendations from the KPIs and anomalies.", () => ({
  recommendations: proposeOptimizations(computeKpis(), detectAnomalies())
}));
addTool(
  "post_correction_entries",
  "Post the correcting entries for the detected anomalies. Sensitive: requires CFO approval.",
  () => {
    postedBatches += 1;
    postedEntries = CORRECTIONS.length;
    return { posted: CORRECTIONS.length };
  },
  { requiresApproval: true, permissions: ["accounting:write"] }
);

// The registry tells the agent WHAT it may call. On the catalog path the graph definition is
// data (no closures), so the code behind each tool name is passed per run as bindings.
const toolBindings = tools.list().map((definition) => ({
  name: definition.name,
  execute: async (input: unknown) => tools.resolve(definition.id)?.handler(input)
}));

// ── The graph: a finance analyst, then a report writer ───────────────────────
const sonnet = model.anthropic("claude-sonnet-4-6");

const app = createGraph({ name: "finance-flow-optimization" })
  .agentNode("finance-analyst", {
    model: sonnet,
    prompt: {
      system:
        "You are a finance analyst. Analyse the Sage export with your tools, propose " +
        "optimizations, then post the correcting entries. Posting requires CFO approval."
    },
    tools,
    suspendForApproval: true,
    maxIterations: 8,
    outputChannel: "analysis"
  })
  .agentNode("report-writer", {
    model: sonnet,
    prompt: { system: "Write a short executive summary of the finance analysis for the CFO." },
    visibleChannels: ["analysis"],
    maxIterations: 2,
    outputChannel: "report"
  })
  .edge("finance-analyst", "report-writer")
  .compile();

const RUN_ID = "run_finance_sage_demo" as RunId;
const approvals = new InMemoryApprovalEngine(); // use a persistent ApprovalEngine in production

// ── Act 1: analysis, then a suspension before any entry is posted ────────────
console.log("\nAct 1 — analysing the Sage export (25 entries):");
const suspended = await runCatalogGraph(app.definition, {
  runId: RUN_ID,
  tools: toolBindings,
  approvalEngine: approvals // files one approval request per gated tool call
});

check(suspended.status === "suspended", "the run suspended before posting correcting entries");
check(String(suspended.state.currentNodeId) === "finance-analyst", "it is paused at the finance-analyst agent");
check(postedBatches === 0, "no correcting entry was posted before approval");

const [request] = await approvals.getPending(RUN_ID);
if (request === undefined) throw new Error("Check failed: no pending approval request was filed");
check(
  "description" in request.subject && request.subject.description === "tool:post_correction_entries",
  "the pending request is for tool:post_correction_entries"
);
check(request.requestedBy === "finance-analyst", "the request was filed by the finance-analyst agent");

// What the analysis tools find in this export (deterministic, whatever the model does):
const kpis = computeKpis();
const anomalies = detectAnomalies();
const recommendations = proposeOptimizations(kpis, anomalies);
check(kpis.avgDaysToCollect >= 50, `high DSO detected (${kpis.avgDaysToCollect} days on average)`);
check(kpis.lateCollections === 3, "3 customer invoices collected 60 or more days late");
check(kpis.manualEntriesPct > 50, `${kpis.manualEntriesPct}% manual entries (> 50%)`);
check(kpis.potentialDuplicates === 1, "1 potential duplicate supplier invoice");
check(anomalies.some((a) => a.type === "duplicate_supplier_invoice"), "the duplicate supplier invoice is flagged");
check(anomalies.some((a) => a.type === "vat_mismatch"), "the VAT mismatch is flagged");
check(anomalies.some((a) => a.type === "suspicious_round_amount"), "the suspicious round transfer is flagged");
check(recommendations.length >= 5, "at least 5 recommendations");

// ── Act 2: the CFO approves; the corrections are posted; the report is written ─
console.log("\nAct 2 — the CFO approves the correcting entries:");
await approvals.approve(request.id, "cfo");
const done = await resumeCatalogGraph(app.definition, suspended.state, {
  tools: toolBindings,
  approvalEngine: approvals,
  // The grant carries its provenance; the engine re-checks it and refuses a self-approval.
  approvedTools: [{ name: "post_correction_entries", requestedBy: request.requestedBy, resolvedBy: "cfo" }]
});

const report = finalAnswer(done.state.channels.report as AgentResult | undefined);
check(done.status === "completed", "the run completed after the CFO's approval");
check(postedBatches === 1, "the correcting entries were posted exactly once");
check(postedEntries === 2, "2 correcting entries posted (duplicate reversal + VAT fix)");
check(report.length > 0, "the report writer produced the executive summary");

// ── Optimization report ──────────────────────────────────────────────────────
console.log("\n════════════════════════════════════════════════════════");
console.log("            FINANCE-FLOW OPTIMIZATION REPORT");
console.log("════════════════════════════════════════════════════════");
console.log(`\nTools the analyst called: ${[...toolsCalled].join(", ")}`);
console.log("\nKPIs:");
console.log(`  Average days to collect (DSO) .. ${kpis.avgDaysToCollect} days`);
console.log(`  Collections ≥ 60 days .......... ${kpis.lateCollections}`);
console.log(`  Manual entries ................. ${kpis.manualEntriesPct}%`);
console.log(`  Matched entries ................ ${kpis.matchedPct}%`);
console.log(`  Potential duplicates ........... ${kpis.potentialDuplicates}`);
console.log("\nAnomalies:");
for (const anomaly of anomalies) {
  console.log(`  [${anomaly.severity}] ${anomaly.type} — ${anomaly.detail}`);
}
console.log("\nRecommendations:");
for (const recommendation of recommendations) {
  console.log(`  • ${recommendation}`);
}
console.log("\nCorrecting entries (approved by the CFO):");
for (const correction of CORRECTIONS) {
  console.log(`  • ${correction.account} — ${correction.label}`);
}
console.log(`\nExecutive summary: ${report}`);

console.log("\nAll checks passed — finance flows optimized under governance.");
