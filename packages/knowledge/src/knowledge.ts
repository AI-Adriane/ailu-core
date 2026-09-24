/**
 * Knowledge base + knowledge graph for Ailu — the engine-owned model, the pure graph
 * operations, and the {@link KnowledgeStore} seam. The TypeScript twin of the Rust
 * `ailu-knowledge` crate.
 *
 * Engine (OSS): the data shapes, the pure ops ({@link buildEdges}, {@link buildGraph},
 * {@link neighbors}, {@link resolveTargetId}), the {@link KnowledgeStore} interface, and
 * {@link InMemoryKnowledgeStore}. The control plane (product) implements a Postgres-backed
 * store and owns tenancy/permissions/connectors/activation — reusing these ops.
 */

/**
 * A typed OKF relation (`<type>:<target>`) — structurally the `relations` entries
 * `@ailu/okf` parses. Defined here (not imported) so this package stays a leaf with
 * no engine-package deps; an OKF `ParsedOkf["relations"]` value is assignable as-is.
 */
export type OkfRelation = { type: string; target: string };

/** A document stored in the knowledge base (its embedding is carried separately). */
export type KbDocument = {
  id: string;
  namespace: string;
  content: string;
  type: string;
  title?: string;
  description?: string;
  resource?: string;
  timestamp?: string;
  path?: string;
  tags?: string[];
  links?: string[];
  createdAt: string;
};

/** A typed edge of the knowledge graph: `from --type--> to`. */
export type KbGraphEdge = { from: string; to: string; type: string };

/** A node of the knowledge graph (a document projected for graph views). */
export type KbGraphNode = { id: string; type: string; title?: string };

/** A namespace's knowledge graph. */
export type KbGraph = { nodes: KbGraphNode[]; edges: KbGraphEdge[] };

/** A single semantic-search hit: the matched document and its cosine score. */
export type KbSearchHit = { id: string; content: string; score: number; type?: string; title?: string };

/** A stored relation (namespaced edge) for backends that key edges by namespace. */
export type KbRelation = { namespace: string; fromId: string; type: string; toId: string };

/**
 * Resolve an OKF link/relation target (bundle-relative `/x.md` or relative) to an entity
 * id within `namespace` (`<namespace>:<path>`). Mirrors the control plane's resolver.
 */
export const resolveTargetId = (namespace: string, target: string): string => {
  const path = target.startsWith("/") ? target.slice(1) : target;
  return `${namespace}:${path}`;
};

const edgeKey = (edge: KbGraphEdge): string => `${edge.from}|${edge.type}|${edge.to}`;

/**
 * Build a document's outgoing edges from its OKF `links` (untyped → `"references"`) and
 * typed `relations`, resolving each target to a node id and de-duplicating by
 * `from|type|to` (first-seen order preserved).
 */
export const buildEdges = (
  namespace: string,
  fromId: string,
  links: string[],
  relations: OkfRelation[]
): KbGraphEdge[] => {
  const candidates: Array<{ type: string; target: string }> = [
    ...links.map((target) => ({ type: "references", target })),
    ...relations.map((relation) => ({ type: relation.type, target: relation.target }))
  ];
  const edges: KbGraphEdge[] = [];
  const seen = new Set<string>();
  for (const candidate of candidates) {
    const edge: KbGraphEdge = {
      from: fromId,
      to: resolveTargetId(namespace, candidate.target),
      type: candidate.type
    };
    const key = edgeKey(edge);
    if (!seen.has(key)) {
      seen.add(key);
      edges.push(edge);
    }
  }
  return edges;
};

/** Assemble a knowledge graph from a namespace's documents (nodes) and edges. */
export const buildGraph = (docs: KbDocument[], edges: KbGraphEdge[]): KbGraph => ({
  nodes: docs.map((doc) => ({
    id: doc.id,
    type: doc.type,
    ...(doc.title !== undefined ? { title: doc.title } : {})
  })),
  edges: [...edges]
});

/** A depth-limited subgraph reachable from `id` by following outgoing edges (BFS). */
export const neighbors = (graph: KbGraph, id: string, depth: number): KbGraph => {
  const adjacency = new Map<string, KbGraphEdge[]>();
  for (const edge of graph.edges) {
    const list = adjacency.get(edge.from) ?? [];
    list.push(edge);
    adjacency.set(edge.from, list);
  }
  const reached = new Set<string>([id]);
  let frontier = [id];
  for (let d = 0; d < depth; d += 1) {
    const next: string[] = [];
    for (const nodeId of frontier) {
      for (const edge of adjacency.get(nodeId) ?? []) {
        if (!reached.has(edge.to)) {
          reached.add(edge.to);
          next.push(edge.to);
        }
      }
    }
    if (next.length === 0) {
      break;
    }
    frontier = next;
  }
  return {
    nodes: graph.nodes.filter((node) => reached.has(node.id)),
    edges: graph.edges.filter((edge) => reached.has(edge.from) && reached.has(edge.to))
  };
};

/** Cosine similarity between two equal-length vectors; 0 when degenerate or mismatched. */
export const cosineSimilarity = (a: number[], b: number[]): number => {
  if (a.length !== b.length || a.length === 0) {
    return 0;
  }
  let dot = 0;
  let normA = 0;
  let normB = 0;
  for (let i = 0; i < a.length; i += 1) {
    const x = a[i] ?? 0;
    const y = b[i] ?? 0;
    dot += x * y;
    normA += x * x;
    normB += y * y;
  }
  if (normA === 0 || normB === 0) {
    return 0;
  }
  return dot / (Math.sqrt(normA) * Math.sqrt(normB));
};

/**
 * Persistence seam for the knowledge base + graph. The engine ships
 * {@link InMemoryKnowledgeStore}; the control plane implements a Postgres-backed one and
 * reuses the pure ops above.
 */
export interface KnowledgeStore {
  putDocument(document: KbDocument, embedding: number[]): Promise<void>;
  getDocument(namespace: string, id: string): Promise<KbDocument | undefined>;
  listDocuments(namespace: string): Promise<KbDocument[]>;
  setRelations(namespace: string, fromId: string, edges: KbGraphEdge[]): Promise<void>;
  listRelations(namespace: string): Promise<KbGraphEdge[]>;
  search(namespace: string, queryEmbedding: number[], k: number): Promise<KbSearchHit[]>;
  graph(namespace: string): Promise<KbGraph>;
  neighbors(namespace: string, id: string, depth: number): Promise<KbGraph>;
}

/** In-memory {@link KnowledgeStore} — cosine search + graph traversal. Dev/test/standalone. */
export class InMemoryKnowledgeStore implements KnowledgeStore {
  private readonly docs: Array<{ document: KbDocument; embedding: number[] }> = [];
  private edges: Array<{ namespace: string; edge: KbGraphEdge }> = [];

  public async putDocument(document: KbDocument, embedding: number[]): Promise<void> {
    const slot = this.docs.find(
      (entry) => entry.document.id === document.id && entry.document.namespace === document.namespace
    );
    if (slot !== undefined) {
      slot.document = document;
      slot.embedding = embedding;
    } else {
      this.docs.push({ document, embedding });
    }
  }

  public async getDocument(namespace: string, id: string): Promise<KbDocument | undefined> {
    return this.docs.find((entry) => entry.document.namespace === namespace && entry.document.id === id)
      ?.document;
  }

  public async listDocuments(namespace: string): Promise<KbDocument[]> {
    return this.docs
      .filter((entry) => entry.document.namespace === namespace)
      .map((entry) => entry.document);
  }

  public async setRelations(namespace: string, fromId: string, edges: KbGraphEdge[]): Promise<void> {
    // Drop this document's prior edges in the namespace, then append the new set (deduped).
    this.edges = this.edges.filter((e) => !(e.namespace === namespace && e.edge.from === fromId));
    const seen = new Set(
      this.edges.filter((e) => e.namespace === namespace).map((e) => edgeKey(e.edge))
    );
    for (const edge of edges) {
      if (!seen.has(edgeKey(edge))) {
        seen.add(edgeKey(edge));
        this.edges.push({ namespace, edge });
      }
    }
  }

  public async listRelations(namespace: string): Promise<KbGraphEdge[]> {
    return this.edges.filter((e) => e.namespace === namespace).map((e) => e.edge);
  }

  public async search(namespace: string, queryEmbedding: number[], k: number): Promise<KbSearchHit[]> {
    return this.docs
      .filter((entry) => entry.document.namespace === namespace)
      .map((entry) => ({
        id: entry.document.id,
        content: entry.document.content,
        score: cosineSimilarity(queryEmbedding, entry.embedding),
        type: entry.document.type,
        ...(entry.document.title !== undefined ? { title: entry.document.title } : {})
      }))
      .sort((a, b) => b.score - a.score)
      .slice(0, k);
  }

  public async graph(namespace: string): Promise<KbGraph> {
    return buildGraph(await this.listDocuments(namespace), await this.listRelations(namespace));
  }

  public async neighbors(namespace: string, id: string, depth: number): Promise<KbGraph> {
    return neighbors(await this.graph(namespace), id, depth);
  }
}
