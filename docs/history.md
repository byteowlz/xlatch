# Optional routing history

Dataset capture is off by default and captures only newly accepted jobs after opt-in. Operational jobs already retain input/results independently; disabling or purging this dataset does not erase them. Retry IDs remain intact, so clearing history cannot cause an offline share to execute twice.

Create a JSON or TOML policy, for example:

```toml
capture = "metadata"
retention_days = 30
max_rows = 10000
max_bytes = 67108864
excluded_capabilities = ["private-notes"]
```

```sh
xlatch history status
xlatch history configure history.toml
xlatch history export --owner DEVICE_ID > routing.jsonl
xlatch history purge --owner DEVICE_ID
```

These commands require direct access to the private server data directory (use the same `--data-dir`). Protected installations therefore require the service administrator. Device/agent invocation credentials cannot change policy or export other devices' records. Omitting `--owner` exports/purges all owners as the trusted operator.

Modes: `off`, `metadata`, `content`. Metadata records version, server/device identity, job ID, exact capability revision, target label, timestamp and encoded input size. Content mode additionally stores input JSON with `base64`/`data_base64` attachment fields omitted; arbitrary text may contain secrets. There is no claim of automatic safe redaction. Never enable content capture on sensitive targets without deliberate consent. Transport credentials and signed envelopes are not captured. Capture is local; nothing is trained or uploaded automatically.

Exports are stable JSONL ordered by timestamp/job ID, with current job outcome. Selection provenance is `unknown` until clients explicitly report it. Success is not a correctness label, and unchosen targets are not negative examples. Policy exclusions affect future capture; purge existing records separately.

Time, row and logical serialized-byte limits apply on capture, export and once-per-minute maintenance. An offline daemon enforces expiry when next used. Purge is logical deletion, not forensic secure erasure of SQLite pages/WAL, backups or copied exports. Export files are owned by the operator, not managed by retention. Operational payload retention and richer provenance/redaction remain tracked separately.
