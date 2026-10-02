"""Embeddings and a vector store — the TypeScript ``createEmbeddings`` / ``createVectorStore``
counterparts (ADR 0045 M4).

The engine builds the embeddings request, makes the HTTP call (Mistral or OpenAI
``/embeddings``), reads the response, and ranks stored vectors by cosine similarity; this module
keeps the items and the file they are saved to. The persisted file is the JSON array the
TypeScript store writes, so a store saved by one SDK loads in the other.
"""

from __future__ import annotations

import json
import os
from typing import Any, Callable, Dict, List, Mapping, Optional

from . import RunError, _native


def _engine(call: Callable[..., Any], *args: Any) -> Any:
    try:
        return call(*args)
    except ValueError as error:
        raise RunError(str(error)) from error


class Embeddings:
    """Turns texts into vectors; see :func:`create_embeddings`."""

    def __init__(
        self,
        options: Dict[str, Any],
        transport: Optional[Callable[[Dict[str, Any]], Any]],
    ) -> None:
        self._options = options
        self._transport = transport

    def embed(self, texts: List[str]) -> List[List[float]]:
        """One vector per text, in order. No texts: no call."""
        texts = list(texts)
        if not texts:
            return []
        options_json = json.dumps(self._options)
        texts_json = json.dumps(texts)
        if self._transport is None:
            return json.loads(_engine(_native.engine_embed, options_json, texts_json))
        body = json.loads(_engine(_native.engine_embeddings_body, options_json, texts_json))
        response = self._transport(body)
        return json.loads(_engine(_native.engine_parse_embeddings_response, json.dumps(response)))


def create_embeddings(
    *,
    provider: str = "mistral",
    api_key: Optional[str] = None,
    model: Optional[str] = None,
    base_url: Optional[str] = None,
    dimensions: Optional[int] = None,
    transport: Optional[Callable[[Dict[str, Any]], Any]] = None,
) -> Embeddings:
    """An embeddings client (the TypeScript ``createEmbeddings``).

    Args:
        provider: ``"mistral"`` (``mistral-embed``) or ``"openai"``
            (``text-embedding-3-small``).
        api_key: The key; else ``MISTRAL_API_KEY`` / ``OPENAI_API_KEY``.
        model: Another embedding model of the provider.
        base_url: Another API base URL.
        dimensions: Down-project the vectors (OpenAI ``text-embedding-3-*``).
        transport: ``fn(body) -> response`` instead of the HTTP call: it
            receives ``{"model", "input", "dimensions"?}`` and returns the
            provider's parsed JSON (``{"data": [{"embedding": [...]}]}``) — for
            offline tests or your own client. No key is needed then.

    Raises at :meth:`Embeddings.embed`: :class:`ailu.RunError` for a missing key
    (naming the variable), a failed request or a malformed response.
    """
    options: Dict[str, Any] = {"provider": provider}
    for key, value in (
        ("apiKey", api_key),
        ("model", model),
        ("baseUrl", base_url),
        ("dimensions", dimensions),
    ):
        if value is not None:
            options[key] = value
    return Embeddings(options, transport)


def cosine_similarity(a: List[float], b: List[float]) -> float:
    """Cosine similarity of two vectors (``0`` when either is all zeros), as in TypeScript."""
    return _engine(_native.engine_cosine_similarity, json.dumps(list(a)), json.dumps(list(b)))


def _is_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def _coerce_item(raw: Any) -> Optional[Dict[str, Any]]:
    """A persisted entry as an item, or ``None`` when it is not one."""
    if not isinstance(raw, Mapping):
        return None
    item_id, content, embedding = raw.get("id"), raw.get("content"), raw.get("embedding")
    if not isinstance(item_id, str) or not isinstance(content, str):
        return None
    if not isinstance(embedding, list) or not all(_is_number(value) for value in embedding):
        return None
    item: Dict[str, Any] = {"id": item_id, "content": content, "embedding": list(embedding)}
    if isinstance(raw.get("metadata"), Mapping):
        item["metadata"] = dict(raw["metadata"])
    return item


class VectorStore:
    """Items ``{"id", "content", "embedding", "metadata"?}`` searched by cosine similarity;
    see :func:`create_vector_store`."""

    def __init__(self, persist_path: Optional[str]) -> None:
        self._persist_path = persist_path
        self._items: Dict[str, Dict[str, Any]] = {}
        if persist_path is not None and os.path.exists(persist_path):
            try:
                with open(persist_path, encoding="utf-8") as stored:
                    parsed = json.load(stored)
            except (OSError, ValueError):
                parsed = []
            for raw in parsed if isinstance(parsed, list) else []:
                item = _coerce_item(raw)
                if item is not None:
                    self._items[item["id"]] = item

    def upsert(self, items: List[Mapping[str, Any]]) -> None:
        """Insert items, or replace them by ``id`` (keeping their place); saves the file."""
        for item in items:
            stored: Dict[str, Any] = {
                "id": item["id"],
                "content": item["content"],
                "embedding": list(item["embedding"]),
            }
            if "metadata" in item:
                stored["metadata"] = item["metadata"]
            self._items[stored["id"]] = stored
        if self._persist_path is not None:
            directory = os.path.dirname(self._persist_path)
            if directory:
                os.makedirs(directory, exist_ok=True)
            with open(self._persist_path, "w", encoding="utf-8") as stored_file:
                json.dump(list(self._items.values()), stored_file)

    def query(self, embedding: List[float], k: int) -> List[Dict[str, Any]]:
        """The ``k`` items closest to ``embedding``: ``{"id", "content", "score", "metadata"?}``,
        highest score first, ties in insertion order."""
        return json.loads(
            _engine(
                _native.engine_query_vectors,
                json.dumps(list(self._items.values())),
                json.dumps(list(embedding)),
                int(k),
            )
        )

    def size(self) -> int:
        """How many items the store holds."""
        return len(self._items)


def create_vector_store(persist_path: Optional[str] = None) -> VectorStore:
    """A vector store (the TypeScript ``createVectorStore``): in memory, or loaded from and saved
    to ``persist_path`` (a JSON array of items) on every upsert."""
    return VectorStore(persist_path)
