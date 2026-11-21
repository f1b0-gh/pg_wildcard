<!-- SPDX-License-Identifier: Apache-2.0 -->
# Compatibility fixtures

These locally generated inputs are required regression fixtures. Do not regenerate
them with a newer writer: tests exercise old-format decoding and lossless
re-encoding. They contain synthetic data, not external telemetry.

- `legacy-v1.puffin`: a version-1 row-group index (1,255 bytes).
- `identity-v1.puffin`: a version-1 identity index with two rows, raw/winpath views,
  snapshot 42, sequence 3 and a synthetic `s3://warehouse/legacy.parquet` path.
  One identity contains a backslash, NUL and a non-ASCII character.

Both fixtures are included in source packages so the extracted library's tests
can run independently of the parent repository.
