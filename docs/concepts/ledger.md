# Ledger

How does a session transcript stay trustworthy across crashes, concurrent
writers, and hand edits? The ledger protocol in `rust/session/store.rs`:
every append re-reads and re-validates the whole file, every read replays
the same validation, and every violation names its line.

## Append

`transcript.jsonl` grows by exactly one line per event:

1. Read and validate the entire ledger. A ledger whose tail was recovered
   (below) refuses immediately:
   `cannot append <dir>: transcript has a recovered truncated tail; resume
   into a child session`.
2. If the file contained legacy (pre-v2) lines, rewrite it as v2 first —
   through a temp file `.transcript.jsonl.migrate-<pid>`, `sync_all`,
   rename, directory sync.
3. Build the new [envelope](event-envelope.md): `sequence = len + 1`,
   `parentId` = current tip (or an explicit parent for a rewind),
   `causationId` = parent, `correlationId` inherited (first event uses its
   own id), fresh outbox seeds, then `seal()` the checksum.
4. Validate the new event against the prior events (the same `validate_next`
   used on read), append the line, and `sync_data` the file.
5. Mirror `state.json.activeLeaf` — temp file
   `.state.json.active-leaf-<12-char nonce>`, write, `sync_all`, rename,
   directory sync. If the mirror fails after the line was written, the error
   says so exactly:
   `event <id> committed, but active leaf mirror update failed: <error>`;
   `reconcile_active_leaf` repairs the mirror from the ledger on next open.

## Validation invariants

`validate_next` refuses with these exact sentences (each prefixed
`<path>:<line>`):

| Invariant | Refusal |
|---|---|
| Sequence is exactly `len + 1` | `breaks ledger sequence: <n>, expected <m>` |
| Event id is fresh | `duplicates event id <id>` |
| Parent is the active leaf (non-rewind) | `breaks ledger lineage: parent <p>, active leaf <l>` |
| Session id matches `state.json` | `belongs to session <a>, expected <b>` |
| Causation equals parent | `has invalid causation` |
| Correlation id non-empty | `has empty correlation id` |
| Outbox seeds are exactly the pending set for all four consumers | `has invalid transactional outbox seeds` |

Rewind events get four extra checks:

| Invariant | Refusal |
|---|---|
| Payload parses | `has invalid rewind payload: <error>` |
| `fromLeafId` is the current active leaf | `rewind source <id> is not active leaf <leaf>` |
| `parentId` equals the named checkpoint | `rewind parent <p> does not match checkpoint <c>` |
| The checkpoint exists | `checkpoint not found: <id>` |
| …and is a `checkpoint` event | `event <id> is not a checkpoint` |
| …and is an ancestor of the active leaf | `checkpoint <id> is not an ancestor of active leaf <leaf>` |

## Read and recovery

Reads verify every line: schema version, checksum, then `validate_next`
replay. Failure modes:

- A malformed line in the middle is fatal:
  `<path>:<line> is malformed JSON: <error>` or
  `<path>:<line> is not a valid V2 event: <error>`.
- A torn **final** line (the file does not end in a newline and the last
  chunk fails to parse) is recovered: the ledger reads up to the last valid
  event and marks `recoveredTruncatedTail: true` in `jeden export`. Such a
  ledger accepts no further appends — resume into a child session. This was
  exercised live in
  [walkthrough-session-export](../walkthrough-session-export.md).
- Legacy v1 lines (`{"version":1,...}`) and pre-versioned lines
  (`{"ts","type","data"}`) migrate in memory (`legacy-<sequence>` ids for
  the latter); the file is rewritten as v2 on the next append. An
  unsupported legacy version refuses with
  `unsupported legacy ledger version <n>`.

## The ledger is a tree

`/checkpoint [label]` appends a `checkpoint` event carrying the exact
model-visible message window. `/rewind <checkpoint-event-id>` appends a
`rewind` event whose parent is the checkpoint — the active lineage jumps
there while abandoned events remain on disk. `active_lineage` walks tip →
root and refuses corrupt graphs:

- `session lineage contains a cycle at <id>`
- `session lineage references missing event <id>`

## Durability discipline

Every mutation follows the same order: write to a create-new temp file (or
append), flush, `fsync` (`sync_data` for appends, `sync_all` for rewrites),
rename over the target where applicable, then `fsync` the directory (unix).
A crash at any point leaves either the old document or a torn tail the
reader recovers — never a silently different history.

## Not to be confused with

- **The [outbox](outbox.md)** — delivery state layered next to the ledger in
  `outbox.jsonl`; the ledger holds only the immutable seeds.
- **The [usage record](usage-record.md)** — accounting, not history; it can
  be reset (`/usage reset`), the ledger cannot.
- **The headless replay log** — bounded per-connection event replay under
  `.jeden/headless/replay/`; see [headless](../headless.md).
