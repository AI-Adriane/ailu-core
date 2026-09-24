/**
 * Tutoriel — Optimisation des flux finance à partir d'un export Sage (avancé).
 *
 * What you'll learn (scenario in French, code comments in English):
 *   - feeding a realistic dataset (a mock Sage journal export with planted issues)
 *     to an agent through plain tools: parsing, KPIs, anomaly detection, optimization
 *     proposals
 *   - the governance core: posting corrective entries is approval-gated; the run
 *     suspends through an ApprovalEngine until the DAF (CFO) approves, then resumes
 *     and posts the corrections exactly once
 *
 * Planted issues in the export: a duplicated supplier invoice, customer invoices
 * paid 60+ days late (high DSO), >50% manual entries, unlettered entries, one
 * suspicious round transfer, one VAT line inconsistent with its base.
 *
 * Offline and self-verifying: scripted mock LLM (no API key); every claim is
 * asserted and the process exits 1 on the first failure.
 *
 * Run it:
 *   pnpm --filter @ailu/graph-sdk example:finance
 */
import {
  createGraph,
  DefaultLLMGateway,
  InMemoryToolRegistry,
  MockLLMProviderAdapter,
  type LLMGateway,
  type LLMResponse,
  type RunId,
  type ToolId
} from "@ailu/graph-sdk";
// Import the in-memory engine directly (not the package index) so the example never
// pulls the Pg engine and its `db`/`pg` dependency chain.
import { InMemoryApprovalEngine } from "../../approval-engine/src/in-memory-approval-engine.js";

// ── Self-verification helpers ────────────────────────────────────────────────
const assert = (condition: boolean, label: string): void => {
  if (!condition) {
    console.error(`✗ ÉCHEC DE L'ASSERTION : ${label}`);
    process.exit(1);
  }
  console.log(`  ✓ ${label}`);
};

const must = <T>(value: T | undefined, label: string): T => {
  if (value === undefined) {
    console.error(`✗ ÉCHEC DE L'ASSERTION : ${label} (valeur absente)`);
    process.exit(1);
  }
  return value;
};

// ── The mock Sage export (25 journal entries, issues planted on purpose) ─────
type EcritureSage = {
  journal: "VEN" | "ACH" | "BAN";
  date: string;
  compte: string;
  libelle: string;
  debit: number;
  credit: number;
  lettrage: string | null;
  source: "manuelle" | "import";
  fournisseur?: string;
  client?: string;
};

const EXPORT_SAGE: EcritureSage[] = [
  // VEN — customer invoices (receivables on 411*)
  { journal: "VEN", date: "2026-01-05", compte: "411AUB", libelle: "Facture F-C001 Maison Aubert", debit: 1200, credit: 0, lettrage: "AA", source: "import", client: "Maison Aubert" },
  { journal: "VEN", date: "2026-01-12", compte: "411BRU", libelle: "Facture F-C002 Atelier Brun", debit: 980, credit: 0, lettrage: "AB", source: "manuelle", client: "Atelier Brun" },
  { journal: "VEN", date: "2026-01-20", compte: "411COS", libelle: "Facture F-C003 Galerie Costa", debit: 2400, credit: 0, lettrage: "AC", source: "manuelle", client: "Galerie Costa" },
  { journal: "VEN", date: "2026-02-03", compte: "411ERR", libelle: "Facture F-C004 Domaine Errel", debit: 1750, credit: 0, lettrage: "AD", source: "import", client: "Domaine Errel" },
  { journal: "VEN", date: "2026-02-10", compte: "411FAG", libelle: "Facture F-C005 Librairie Fage", debit: 640, credit: 0, lettrage: null, source: "manuelle", client: "Librairie Fage" },
  { journal: "VEN", date: "2026-02-18", compte: "411AUB", libelle: "Facture F-C006 Maison Aubert", debit: 3100, credit: 0, lettrage: null, source: "manuelle", client: "Maison Aubert" },
  // BAN — bank: customer receipts (lettrage links a receipt to its invoice)
  { journal: "BAN", date: "2026-01-25", compte: "512", libelle: "Encaissement F-C001", debit: 1200, credit: 0, lettrage: "AA", source: "import" },
  { journal: "BAN", date: "2026-03-20", compte: "512", libelle: "Encaissement F-C002", debit: 980, credit: 0, lettrage: "AB", source: "import" },
  { journal: "BAN", date: "2026-03-28", compte: "512", libelle: "Encaissement F-C003", debit: 2400, credit: 0, lettrage: "AC", source: "import" },
  { journal: "BAN", date: "2026-04-15", compte: "512", libelle: "Encaissement F-C004", debit: 1750, credit: 0, lettrage: "AD", source: "import" },
  { journal: "BAN", date: "2026-02-28", compte: "512", libelle: "Virement interne", debit: 10000, credit: 0, lettrage: null, source: "manuelle" },
  { journal: "BAN", date: "2026-03-31", compte: "627", libelle: "Frais bancaires mars", debit: 0, credit: 38.5, lettrage: null, source: "import" },
  // ACH — supplier invoices (payables on 401*)
  { journal: "ACH", date: "2026-01-08", compte: "401LUT", libelle: "Facture Papeterie Lutece - fournitures", debit: 0, credit: 850, lettrage: "BA", source: "manuelle", fournisseur: "Papeterie Lutece" },
  { journal: "ACH", date: "2026-01-15", compte: "401MAR", libelle: "Facture Transports Marek - livraisons", debit: 0, credit: 1320, lettrage: "BB", source: "import", fournisseur: "Transports Marek" },
  { journal: "ACH", date: "2026-02-02", compte: "401HEL", libelle: "Prestation maquettes printemps", debit: 0, credit: 2150, lettrage: null, source: "manuelle", fournisseur: "Studio Helio" },
  { journal: "ACH", date: "2026-02-09", compte: "401HEL", libelle: "Prestation maquettes printemps", debit: 0, credit: 2150, lettrage: null, source: "manuelle", fournisseur: "Studio Helio" },
  { journal: "ACH", date: "2026-02-12", compte: "607", libelle: "Achat presentoirs - HT", debit: 1000, credit: 0, lettrage: null, source: "manuelle", fournisseur: "Mobilier Kova" },
  { journal: "ACH", date: "2026-02-12", compte: "44566", libelle: "TVA deductible presentoirs (20%)", debit: 250, credit: 0, lettrage: null, source: "manuelle", fournisseur: "Mobilier Kova" },
  { journal: "ACH", date: "2026-02-12", compte: "401KOV", libelle: "Facture Mobilier Kova - presentoirs TTC", debit: 0, credit: 1250, lettrage: null, source: "manuelle", fournisseur: "Mobilier Kova" },
  { journal: "ACH", date: "2026-02-20", compte: "401ENE", libelle: "Facture Energie Roule - electricite", debit: 0, credit: 410, lettrage: "BC", source: "import", fournisseur: "Energie Roule" },
  { journal: "ACH", date: "2026-03-01", compte: "401IMP", libelle: "Facture Imprimerie Sel - catalogues", debit: 0, credit: 760, lettrage: null, source: "manuelle", fournisseur: "Imprimerie Sel" },
  { journal: "ACH", date: "2026-03-05", compte: "401NET", libelle: "Facture Nettoyage Pur - mars", debit: 0, credit: 290, lettrage: null, source: "manuelle", fournisseur: "Nettoyage Pur" },
  // BAN — supplier payments
  { journal: "BAN", date: "2026-01-30", compte: "512", libelle: "Reglement Papeterie Lutece", debit: 0, credit: 850, lettrage: "BA", source: "import" },
  { journal: "BAN", date: "2026-02-15", compte: "512", libelle: "Reglement Transports Marek", debit: 0, credit: 1320, lettrage: "BB", source: "manuelle" },
  { journal: "BAN", date: "2026-03-10", compte: "512", libelle: "Reglement Energie Roule", debit: 0, credit: 410, lettrage: "BC", source: "manuelle" }
];

// ── Pure analysis functions (the tools call these; assertions reuse them) ────
type Kpis = {
  dsoMoyenJours: number;
  retardsEncaissement: number;
  partSaisiesManuellesPct: number;
  tauxLettragePct: number;
  doublonsPotentiels: number;
};

type Anomalie = { type: string; gravite: "haute" | "moyenne"; detail: string };

const round2 = (value: number): number => Math.round(value * 100) / 100;

const daysBetween = (from: string, to: string): number =>
  Math.round((new Date(to).getTime() - new Date(from).getTime()) / 86_400_000);

const eur = (montant: number): string =>
  new Intl.NumberFormat("fr-FR", { style: "currency", currency: "EUR" }).format(montant);

const isAuxiliaire = (entry: EcritureSage): boolean =>
  entry.compte.startsWith("411") || entry.compte.startsWith("401");

/** Pair each lettered customer invoice (VEN/411*) with its bank receipt by lettrage code. */
const delaisEncaissement = (): number[] =>
  EXPORT_SAGE.filter((e) => e.journal === "VEN" && e.lettrage !== null).flatMap((facture) => {
    const encaissement = EXPORT_SAGE.find(
      (e) => e.journal === "BAN" && e.lettrage === facture.lettrage
    );
    return encaissement === undefined ? [] : [daysBetween(facture.date, encaissement.date)];
  });

const doublonsFournisseurs = (): Array<{ fournisseur: string; montant: number; libelle: string }> => {
  const groupes = new Map<string, EcritureSage[]>();
  for (const entry of EXPORT_SAGE) {
    if (entry.journal === "ACH" && entry.credit > 0 && entry.fournisseur !== undefined) {
      const key = `${entry.fournisseur}|${entry.credit}|${entry.libelle}`;
      groupes.set(key, [...(groupes.get(key) ?? []), entry]);
    }
  }
  return [...groupes.values()]
    .filter((group) => group.length > 1)
    .flatMap((group) => {
      const first = group[0];
      return first === undefined
        ? []
        : [{ fournisseur: first.fournisseur ?? "?", montant: first.credit, libelle: first.libelle }];
    });
};

const computeKpis = (): Kpis => {
  const delais = delaisEncaissement();
  const totalDelais = delais.reduce((sum, days) => sum + days, 0);
  const auxiliaires = EXPORT_SAGE.filter(isAuxiliaire);
  const lettrees = auxiliaires.filter((e) => e.lettrage !== null);
  const manuelles = EXPORT_SAGE.filter((e) => e.source === "manuelle");
  return {
    dsoMoyenJours: delais.length > 0 ? Math.round(totalDelais / delais.length) : 0,
    retardsEncaissement: delais.filter((days) => days >= 60).length,
    partSaisiesManuellesPct: Math.round((manuelles.length / EXPORT_SAGE.length) * 100),
    tauxLettragePct: Math.round((lettrees.length / auxiliaires.length) * 100),
    doublonsPotentiels: doublonsFournisseurs().length
  };
};

const detectAnomalies = (): Anomalie[] => {
  const anomalies: Anomalie[] = [];

  for (const doublon of doublonsFournisseurs()) {
    anomalies.push({
      type: "doublon_fournisseur",
      gravite: "haute",
      detail:
        `Facture « ${doublon.libelle} » de ${doublon.fournisseur} comptabilisée deux fois ` +
        `(${eur(doublon.montant)})`
    });
  }

  // VAT line vs its base: same supplier + date; expected = base × 20%.
  for (const tva of EXPORT_SAGE.filter((e) => e.compte.startsWith("44566"))) {
    const base = EXPORT_SAGE.find(
      (e) => e.compte.startsWith("607") && e.fournisseur === tva.fournisseur && e.date === tva.date
    );
    if (base !== undefined) {
      const attendu = round2(base.debit * 0.2);
      if (round2(tva.debit) !== attendu) {
        anomalies.push({
          type: "tva_incoherente",
          gravite: "haute",
          detail:
            `TVA déductible de ${eur(tva.debit)} pour une base de ${eur(base.debit)} ` +
            `(attendu ${eur(attendu)} à 20 %) — écart ${eur(tva.debit - attendu)}`
        });
      }
    }
  }

  for (const entry of EXPORT_SAGE) {
    const montant = entry.debit + entry.credit;
    if (entry.journal === "BAN" && entry.source === "manuelle" && montant >= 5000 && montant % 1000 === 0) {
      anomalies.push({
        type: "montant_rond_suspect",
        gravite: "moyenne",
        detail: `« ${entry.libelle} » de ${eur(montant)} saisi manuellement, non lettré (${entry.date})`
      });
    }
  }

  const retards = delaisEncaissement().filter((days) => days >= 60);
  if (retards.length > 0) {
    anomalies.push({
      type: "retard_paiement",
      gravite: "moyenne",
      detail: `${retards.length} factures clients encaissées à 60 jours ou plus (DSO dégradé)`
    });
  }

  const nonLettrees = EXPORT_SAGE.filter((e) => isAuxiliaire(e) && e.lettrage === null);
  if (nonLettrees.length > 0) {
    anomalies.push({
      type: "non_lettre",
      gravite: "moyenne",
      detail: `${nonLettrees.length} écritures clients/fournisseurs non lettrées`
    });
  }

  return anomalies;
};

const proposeOptimizations = (kpis: Kpis, anomalies: Anomalie[]): string[] => {
  const recos: string[] = [];
  if (kpis.partSaisiesManuellesPct > 50) {
    recos.push(
      "Automatiser les imports comptables (connecteur bancaire + OCR fournisseurs) pour " +
        `réduire la part de saisies manuelles (${kpis.partSaisiesManuellesPct} % aujourd'hui).`
    );
  }
  if (kpis.tauxLettragePct < 80) {
    recos.push(
      "Activer le lettrage automatique par référence de facture " +
        `(taux de lettrage actuel : ${kpis.tauxLettragePct} %).`
    );
  }
  if (kpis.dsoMoyenJours > 45) {
    recos.push(
      "Mettre en place des relances clients automatisées (J+15, J+30, J+45) pour réduire " +
        `le DSO (${kpis.dsoMoyenJours} jours en moyenne).`
    );
  }
  if (kpis.doublonsPotentiels > 0) {
    recos.push(
      "Dédoublonner les factures fournisseurs : contrôle fournisseur + montant + libellé à la saisie."
    );
  }
  if (anomalies.some((a) => a.type === "tva_incoherente")) {
    recos.push("Ajouter des contrôles TVA automatiques (cohérence base × taux) avant validation.");
  }
  recos.push("Instaurer un workflow de validation humaine des saisies manuelles au-delà d'un seuil.");
  return recos;
};

// ── Tools wired to the analysis functions (results captured for the report) ──
type Totaux = Array<{ journal: string; lignes: number; totalDebit: number; totalCredit: number }>;
const captured: { totaux?: Totaux; kpis?: Kpis; anomalies?: Anomalie[]; recos?: string[] } = {};
let lotsCorrectifs = 0;
let ecrituresPassees = 0;

const passthrough = { parse: (value: unknown) => value };
const tools = new InMemoryToolRegistry();

tools.register(
  {
    id: "parse_sage_export" as ToolId,
    name: "parse_sage_export",
    description: "Parse l'export Sage : totaux et nombre de lignes par journal.",
    inputSchema: passthrough,
    outputSchema: passthrough,
    permissions: [],
    jsonSchema: { type: "object" }
  },
  async () => {
    const totaux: Totaux = (["VEN", "ACH", "BAN"] as const).map((journal) => {
      const lignes = EXPORT_SAGE.filter((e) => e.journal === journal);
      return {
        journal,
        lignes: lignes.length,
        totalDebit: round2(lignes.reduce((sum, e) => sum + e.debit, 0)),
        totalCredit: round2(lignes.reduce((sum, e) => sum + e.credit, 0))
      };
    });
    captured.totaux = totaux;
    return { totaux, totalLignes: EXPORT_SAGE.length };
  }
);

tools.register(
  {
    id: "compute_kpis" as ToolId,
    name: "compute_kpis",
    description: "Calcule les KPIs : DSO, part de saisies manuelles, taux de lettrage, doublons.",
    inputSchema: passthrough,
    outputSchema: passthrough,
    permissions: [],
    jsonSchema: { type: "object" }
  },
  async () => {
    const kpis = computeKpis();
    captured.kpis = kpis;
    return kpis;
  }
);

tools.register(
  {
    id: "detect_anomalies" as ToolId,
    name: "detect_anomalies",
    description: "Détecte les anomalies : doublons, TVA incohérente, montants ronds, retards.",
    inputSchema: passthrough,
    outputSchema: passthrough,
    permissions: [],
    jsonSchema: { type: "object" }
  },
  async () => {
    const anomalies = detectAnomalies();
    captured.anomalies = anomalies;
    return { anomalies };
  }
);

tools.register(
  {
    id: "propose_optimizations" as ToolId,
    name: "propose_optimizations",
    description: "Dérive des recommandations d'optimisation des KPIs et anomalies.",
    inputSchema: passthrough,
    outputSchema: passthrough,
    permissions: [],
    jsonSchema: { type: "object" }
  },
  async () => {
    const recos = proposeOptimizations(computeKpis(), detectAnomalies());
    captured.recos = recos;
    return { recommandations: recos };
  }
);

tools.register(
  {
    id: "post_correction_entries" as ToolId,
    name: "post_correction_entries",
    description: "Passe des écritures correctives. Sensible — approbation du DAF requise.",
    inputSchema: passthrough,
    outputSchema: passthrough,
    permissions: ["compta:write"],
    requiresApproval: true,
    jsonSchema: {
      type: "object",
      properties: { ecritures: { type: "array" } },
      required: ["ecritures"]
    }
  },
  async (input: unknown) => {
    const { ecritures } = input as { ecritures: Array<{ libelle: string }> };
    lotsCorrectifs += 1;
    ecrituresPassees = ecritures.length;
    return { posted: ecritures.length };
  }
);

// ── Scripted mock LLM ────────────────────────────────────────────────────────
let toolUseSeq = 0;
const toolTurn = (name: string, input: Record<string, unknown> = {}): LLMResponse => ({
  content: "",
  toolCalls: [{ id: `tu_${(toolUseSeq += 1)}`, name, input }],
  stopReason: "tool_use",
  usage: { promptTokens: 0, completionTokens: 0 },
  model: "mock",
  provider: "anthropic"
});

const finalTurn = (content: string): LLMResponse => ({
  content,
  usage: { promptTokens: 0, completionTokens: 0 },
  model: "mock",
  provider: "anthropic"
});

const scripted = (responses: LLMResponse[]): LLMGateway => {
  const gateway = new DefaultLLMGateway();
  gateway.registerAdapter(new MockLLMProviderAdapter({ provider: "anthropic", responses }));
  return gateway;
};

const corrections: Array<Record<string, unknown>> = [
  { compte: "401HEL", libelle: "Extourne doublon Studio Helio (facture comptabilisée 2 fois)", montant: 2150 },
  { compte: "44566", libelle: "Correction TVA présentoirs : 250 → 200 (base 1 000 à 20 %)", montant: -50 }
];

// KEY mock-sequencing rule: the scripted gateway is stateful across suspend/resume.
// The gated tool_use is scripted TWICE: the first suspends the run (approval filed),
// the second — consumed when the resumed agent re-runs — actually posts, then FINAL.
const script: LLMResponse[] = [
  toolTurn("parse_sage_export"),
  toolTurn("compute_kpis"),
  toolTurn("detect_anomalies"),
  toolTurn("propose_optimizations"),
  toolTurn("post_correction_entries", { ecritures: corrections }),
  toolTurn("post_correction_entries", { ecritures: corrections }),
  finalTurn("FINAL: Rapport d'optimisation prêt — écritures correctives passées après approbation du DAF.")
];

// ── The graph: one finance analyst agent, governed by the ApprovalEngine ─────
const engine = new InMemoryApprovalEngine();

const app = createGraph({ name: "optimisation-flux-finance" })
  .channel("rapportPublie", { type: "boolean", default: false })
  .agentNode("analyste-finance", {
    llm: scripted(script),
    prompt: {
      system:
        "Tu es analyste finance. Analyse l'export Sage avec les outils, propose des " +
        "optimisations. Toute écriture corrective exige l'approbation du DAF."
    },
    tools,
    suspendForApproval: true,
    approvalEngine: engine,
    maxIterations: 8,
    outputChannel: "analysis"
  })
  .node("publier-rapport", async () => ({ rapportPublie: true }))
  .edge("analyste-finance", "publier-rapport")
  .compile();

const RUN_ID = "run_finance_sage_demo" as RunId;

// ── Acte 1 : analyse, puis suspension avant les écritures correctives ────────
console.log("\nActe 1 — analyse de l'export Sage (25 écritures) :");
const suspendu = await app.run({}, { runId: RUN_ID });

assert(suspendu.status === "suspended", "le run se suspend avant les écritures correctives");
assert(lotsCorrectifs === 0, "aucune écriture corrective passée avant approbation");

const kpis = must(captured.kpis, "KPIs calculés par compute_kpis");
assert(kpis.dsoMoyenJours >= 50, `DSO élevé détecté (${kpis.dsoMoyenJours} jours en moyenne)`);
assert(kpis.retardsEncaissement === 3, "3 factures clients encaissées à 60 jours ou plus");
assert(kpis.partSaisiesManuellesPct > 50, `${kpis.partSaisiesManuellesPct} % de saisies manuelles (> 50 %)`);
assert(kpis.doublonsPotentiels === 1, "1 doublon fournisseur potentiel identifié");

const anomalies = must(captured.anomalies, "anomalies détectées par detect_anomalies");
assert(anomalies.some((a) => a.type === "doublon_fournisseur"), "le doublon fournisseur est signalé");
assert(anomalies.some((a) => a.type === "tva_incoherente"), "l'incohérence de TVA est signalée");
assert(anomalies.some((a) => a.type === "montant_rond_suspect"), "le virement rond suspect est signalé");
assert(must(captured.recos, "recommandations proposées").length >= 5, "au moins 5 recommandations");

const pending = await engine.getPending(RUN_ID);
assert(pending.length === 1, "exactement 1 demande d'approbation en attente");
const demande = must(pending[0], "la demande d'approbation");
assert(
  JSON.stringify(demande.subject).includes("tool:post_correction_entries"),
  "la demande porte sur tool:post_correction_entries"
);
assert(demande.requestedBy === "analyste-finance", "la demande émane de l'agent analyste-finance");

// ── Acte 2 : le DAF approuve, les corrections passent, le rapport sort ───────
console.log("\nActe 2 — le DAF approuve les écritures correctives :");
await engine.approve(demande.id, "daf");
const termine = await app.resume(RUN_ID);

assert(termine.status === "completed", "le run se termine après l'approbation du DAF");
assert(lotsCorrectifs === 1, "les écritures correctives sont passées exactement une fois");
assert(ecrituresPassees === 2, "2 écritures correctives passées (extourne doublon + TVA)");
assert(termine.channels.rapportPublie, "le rapport est publié");

// ── Rapport d'optimisation ───────────────────────────────────────────────────
const totaux = must(captured.totaux, "totaux par journal");
const recos = must(captured.recos, "recommandations");

console.log("\n════════════════════════════════════════════════════════");
console.log("        RAPPORT D'OPTIMISATION DES FLUX FINANCE");
console.log("════════════════════════════════════════════════════════");
console.log("\nVolumétrie (export Sage) :");
for (const t of totaux) {
  console.log(
    `  ${t.journal} — ${t.lignes} lignes | débit ${eur(t.totalDebit)} | crédit ${eur(t.totalCredit)}`
  );
}
console.log("\nKPIs :");
console.log(`  DSO moyen ................ ${kpis.dsoMoyenJours} jours`);
console.log(`  Encaissements ≥ 60 jours . ${kpis.retardsEncaissement}`);
console.log(`  Saisies manuelles ........ ${kpis.partSaisiesManuellesPct} %`);
console.log(`  Taux de lettrage ......... ${kpis.tauxLettragePct} %`);
console.log(`  Doublons potentiels ...... ${kpis.doublonsPotentiels}`);
console.log("\nAnomalies détectées :");
for (const anomalie of anomalies) {
  console.log(`  [${anomalie.gravite}] ${anomalie.type} — ${anomalie.detail}`);
}
console.log("\nRecommandations :");
for (const reco of recos) {
  console.log(`  • ${reco}`);
}
console.log("\nÉcritures correctives (approuvées par le DAF) :");
for (const ecriture of corrections) {
  console.log(`  • ${String(ecriture.compte)} — ${String(ecriture.libelle)}`);
}

console.log("\nToutes les assertions sont passées — flux finance optimisés sous gouvernance.");
