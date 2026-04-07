#!/usr/bin/env python3
"""Backfill embedding vectors in brain.db using an OpenAI-compatible API.

Serialization format matches the Rust code in src/memory/vector.rs:
little-endian f32 BLOB (4 bytes per dimension).
"""

import argparse
import sqlite3
import struct
import sys
from urllib.parse import urlparse

import requests


def vec_to_bytes(vec: list[float]) -> bytes:
    """Serialize float vector to little-endian f32 bytes (matches Rust vec_to_bytes)."""
    return b"".join(struct.pack("<f", x) for x in vec)


def bytes_to_vec(blob: bytes) -> list[float]:
    """Deserialize little-endian f32 bytes back to floats (matches Rust bytes_to_vec)."""
    count = len(blob) // 4
    return list(struct.unpack(f"<{count}f", blob))


def embeddings_url(base_url: str) -> str:
    """Build the embeddings endpoint URL, matching Rust OpenAiEmbedding logic."""
    base_url = base_url.rstrip("/")
    parsed = urlparse(base_url)
    path = parsed.path.rstrip("/")

    # Already points to /embeddings
    if path.endswith("/embeddings"):
        return base_url

    # Has an explicit API path (not just /)
    if path and path != "/":
        return f"{base_url}/embeddings"

    # Bare domain — prepend /v1
    return f"{base_url}/v1/embeddings"


def call_embedding_api(
    url: str, api_key: str, model: str, texts: list[str]
) -> list[list[float]]:
    """Call the OpenAI-compatible embedding API and return vectors."""
    resp = requests.post(
        url,
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
        },
        json={"model": model, "input": texts},
        timeout=120,
    )

    if not resp.ok:
        print(f"API error {resp.status_code}: {resp.text}", file=sys.stderr)
        sys.exit(1)

    data = resp.json().get("data")
    if not data:
        print("Invalid API response: missing 'data' field", file=sys.stderr)
        sys.exit(1)

    # Sort by index to ensure order matches input
    data.sort(key=lambda item: item.get("index", 0))

    return [item["embedding"] for item in data]


def main():
    parser = argparse.ArgumentParser(
        description="Backfill embeddings in brain.db using an OpenAI-compatible API"
    )
    parser.add_argument(
        "--db", default="memory/brain.db", help="Path to brain.db (default: memory/brain.db)"
    )
    parser.add_argument("--api-url", required=True, help="Embedding API base URL")
    parser.add_argument("--api-key", required=True, help="Bearer token for the API")
    parser.add_argument(
        "--model",
        default="text-embedding-3-small",
        help="Embedding model (default: text-embedding-3-small)",
    )
    parser.add_argument(
        "--batch-size",
        type=int,
        default=50,
        help="Number of texts per API call (default: 50)",
    )
    parser.add_argument(
        "--only-missing",
        action="store_true",
        help="Only embed rows where embedding is NULL",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show what would be done without writing to the database",
    )
    args = parser.parse_args()

    url = embeddings_url(args.api_url)
    print(f"Using endpoint: {url}")
    print(f"Model: {args.model}")

    conn = sqlite3.connect(args.db)
    conn.execute("PRAGMA journal_mode=WAL;")

    if args.only_missing:
        rows = conn.execute(
            "SELECT id, content FROM memories WHERE embedding IS NULL"
        ).fetchall()
    else:
        rows = conn.execute("SELECT id, content FROM memories").fetchall()

    total = len(rows)
    if total == 0:
        print("No memories to process.")
        return

    print(f"Processing {total} memories (batch_size={args.batch_size})...")

    updated = 0
    for i in range(0, total, args.batch_size):
        batch = rows[i : i + args.batch_size]
        ids = [row[0] for row in batch]
        texts = [row[1] for row in batch]

        embeddings = call_embedding_api(url, args.api_key, args.model, texts)

        if len(embeddings) != len(texts):
            print(
                f"Warning: got {len(embeddings)} embeddings for {len(texts)} texts",
                file=sys.stderr,
            )

        for row_id, emb in zip(ids, embeddings):
            blob = vec_to_bytes(emb)
            if args.dry_run:
                dims = len(emb)
                print(f"  [dry-run] {row_id}: {dims} dims, {len(blob)} bytes")
            else:
                conn.execute(
                    "UPDATE memories SET embedding = ? WHERE id = ?", (blob, row_id)
                )

        if not args.dry_run:
            conn.commit()

        updated += len(batch)
        print(f"  {updated}/{total} done")

    conn.close()
    if args.dry_run:
        print(f"Dry run complete. {updated} memories would be updated.")
    else:
        print(f"Done. Updated {updated} memories.")


if __name__ == "__main__":
    main()
