/**
 * A small embedding-backed vector store as an exported SDK helper (NOT a catalog
 * component kind). {@link createVectorStore} keeps `{ id, content, embedding, metadata? }`
 * items and answers nearest-neighbour {@link VectorStore.query} calls by cosine
 * similarity (descending). In-memory by default; when `persistPath` is set the store is
 * persisted to / loaded from a round-trippable JSON file (synchronous `fs`). The exported
 * {@link cosineSimilarity} powers the ranking and is reusable on its own.
 *
 * ```ts
 * import { createVectorStore } from "@ailu/graph-sdk";
 *
 * const store = createVectorStore();
 * store.upsert([{ id: "a", content: "hello", embedding: [1, 0] }]);
 * const hits = store.query([1, 0], 1); // [{ id: "a", content: "hello", score: 1 }]
 * ```
 */

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

/** An item stored in the vector store: an id, its text, its embedding and optional metadata. */
export type VectorStoreItem = {
  id: string;
  content: string;
  embedding: number[];
  metadata?: Record<string, unknown>;
};

/** A single nearest-neighbour result: the item (minus its embedding) plus a cosine score. */
export type VectorStoreMatch = {
  id: string;
  content: string;
  score: number;
  metadata?: Record<string, unknown>;
};

/** The vector store surface returned by {@link createVectorStore}. */
export type VectorStore = {
  /** Insert or replace items by `id` (last write wins); persists when `persistPath` is set. */
  upsert(items: VectorStoreItem[]): void;
  /** Return the top-`k` items by cosine similarity to `embedding`, highest score first. */
  query(embedding: number[], k: number): VectorStoreMatch[];
  /** The number of items currently held. */
  size(): number;
};

/** Options for {@link createVectorStore}. */
export type CreateVectorStoreOptions = {
  /**
   * When set, the store loads its items from this JSON file on creation (if present) and
   * rewrites it on every {@link VectorStore.upsert}. The file is a round-trippable JSON
   * array of {@link VectorStoreItem}. Omit for a purely in-memory store.
   */
  persistPath?: string;
};

/**
 * Cosine similarity of two vectors. Compares over the shorter length (missing
 * components count as 0) and returns `0` when either vector has zero magnitude, so a
 * degenerate input never produces `NaN`.
 */
export const cosineSimilarity = (a: number[], b: number[]): number => {
  const len = Math.min(a.length, b.length);
  let dot = 0;
  let normA = 0;
  let normB = 0;
  for (let i = 0; i < len; i += 1) {
    const av = a[i] ?? 0;
    const bv = b[i] ?? 0;
    dot += av * bv;
    normA += av * av;
    normB += bv * bv;
  }
  // Account for the magnitude of any tail components beyond the common prefix so two
  // different-length vectors still differ in norm.
  for (let i = len; i < a.length; i += 1) {
    const av = a[i] ?? 0;
    normA += av * av;
  }
  for (let i = len; i < b.length; i += 1) {
    const bv = b[i] ?? 0;
    normB += bv * bv;
  }
  if (normA === 0 || normB === 0) {
    return 0;
  }
  return dot / (Math.sqrt(normA) * Math.sqrt(normB));
};

/** Validate one parsed JSON entry into a {@link VectorStoreItem}, or `undefined` if malformed. */
const coerceItem = (raw: unknown): VectorStoreItem | undefined => {
  if (typeof raw !== "object" || raw === null) {
    return undefined;
  }
  const obj = raw as Record<string, unknown>;
  if (typeof obj.id !== "string" || typeof obj.content !== "string") {
    return undefined;
  }
  if (!Array.isArray(obj.embedding) || !obj.embedding.every((n) => typeof n === "number")) {
    return undefined;
  }
  const item: VectorStoreItem = {
    id: obj.id,
    content: obj.content,
    embedding: obj.embedding as number[]
  };
  if (typeof obj.metadata === "object" && obj.metadata !== null && !Array.isArray(obj.metadata)) {
    item.metadata = obj.metadata as Record<string, unknown>;
  }
  return item;
};

/** Load persisted items from `path` (returns `[]` when the file is absent or unparseable). */
const loadItems = (path: string): VectorStoreItem[] => {
  if (!existsSync(path)) {
    return [];
  }
  try {
    const parsed = JSON.parse(readFileSync(path, "utf8")) as unknown;
    if (!Array.isArray(parsed)) {
      return [];
    }
    const items: VectorStoreItem[] = [];
    for (const raw of parsed) {
      const item = coerceItem(raw);
      if (item !== undefined) {
        items.push(item);
      }
    }
    return items;
  } catch {
    return [];
  }
};

/** Persist `items` to `path` as a JSON array, creating the parent directory if needed. */
const persistItems = (path: string, items: VectorStoreItem[]): void => {
  const dir = dirname(path);
  if (dir.length > 0 && !existsSync(dir)) {
    mkdirSync(dir, { recursive: true });
  }
  writeFileSync(path, JSON.stringify(items), "utf8");
};

/**
 * Create a {@link VectorStore}. In-memory by default; with `persistPath` it loads any
 * existing JSON file on creation and rewrites it on every upsert (round-trippable).
 */
export const createVectorStore = (options: CreateVectorStoreOptions = {}): VectorStore => {
  const persistPath = options.persistPath;
  // Keyed by id so upsert is last-write-wins; a Map preserves first-seen order for ties.
  const items = new Map<string, VectorStoreItem>();
  if (persistPath !== undefined) {
    for (const item of loadItems(persistPath)) {
      items.set(item.id, item);
    }
  }

  const flush = (): void => {
    if (persistPath !== undefined) {
      persistItems(persistPath, [...items.values()]);
    }
  };

  return {
    upsert(incoming) {
      for (const item of incoming) {
        // Defensively copy the embedding so a later mutation of the caller's array
        // can't perturb stored vectors.
        const stored: VectorStoreItem = {
          id: item.id,
          content: item.content,
          embedding: [...item.embedding]
        };
        if (item.metadata !== undefined) {
          stored.metadata = item.metadata;
        }
        items.set(item.id, stored);
      }
      flush();
    },
    query(embedding, k) {
      const scored = [...items.values()].map((item, index) => ({
        item,
        index,
        score: cosineSimilarity(embedding, item.embedding)
      }));
      // Descending by score; stable so insertion order breaks ties deterministically.
      scored.sort((a, b) => (b.score - a.score === 0 ? a.index - b.index : b.score - a.score));
      return scored.slice(0, Math.max(0, k)).map(({ item, score }) => {
        const match: VectorStoreMatch = { id: item.id, content: item.content, score };
        if (item.metadata !== undefined) {
          match.metadata = item.metadata;
        }
        return match;
      });
    },
    size() {
      return items.size;
    }
  };
};
