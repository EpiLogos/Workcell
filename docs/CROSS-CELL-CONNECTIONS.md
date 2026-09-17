# Cross-cell connections

A connection is a durable, revocable, expirable, permissioned relation
between two Workcell cells — not a socket. One cell serves its control
plane; another cell connects to it under a grant. Every command below is a
production path: no fixture servers, no credential bypasses.

This document is also the runbook for the acceptance campaign's §5 two-machine
case list (`O-I docs/CONTEXT-FRAME-ACCEPTANCE-CAMPAIGN.md` §5). Each case maps
to the commands whose output proves it, with the evidence fields named.

For human-facing file access to a cell's machine — Finder over the tailnet —
see `REMOTE-FILE-SHARING.md`. That surface is deliberately outside the control
protocol: it serves a person at a desktop, not an agent at a Workcell seam.

## The surface

```text
workcell serve --listen HOST:PORT [--authorization TOKEN]
    Serve this cell's control plane. Grants are enforced per request from the
    connection grants registry. A non-loopback listener requires a token or at
    least one active grant. The bound endpoint is printed to stderr.

workcell authorise --client <label> --allow <operation>... [--advertise <port>...] [--expires-in <duration>] [--store-credential]
    Grant a connecting client named operations. The credential (`wck_…`) is
    shown once; only its SHA-256 is kept (in <state-root>/connections/grants.json).
    An optional `--expires-in <duration>` (`<number><s|m|h|d>`, for example
    `30m`, `12h`, `7d`) sets when the grant stops authorising: past that
    instant it refuses at the client's next use as *expired* — named and
    distinct from revocation — and the record is kept. A grant created
    without `--expires-in` never expires. Zero, negative and malformed
    durations are refused as usage errors.

workcell revoke --client <label> | --grant <ref>
    Revoke grants. Revocation takes effect at the connecting client's next
    use. The grant record is kept as audit evidence.

workcell connect --endpoint HOST:PORT [--connection <label>] [--authorization TOKEN] [--store-credential]
    Establish or reconnect the client side: handshake, compatibility check,
    granted-scope read-back, and probes under the grant. Every outcome —
    connected, refused, incompatible, unreachable — updates the local
    connection record honestly before the loud answer is returned.

workcell connections list | show <label> | disconnect <label>
    Read and reconcile the client-side receipts. `disconnect` keeps the
    record; only the state changes.

workcell --endpoint HOST:PORT --authorization TOKEN <operation> ...
    Ordinary remote operations (status, discover, prepare, observe, …) run
    through the connection. The receipt names the cell that actually
    materialised the world.
```

Design law worth keeping in mind while reading evidence:

- Capability advertisement is not authorisation. Discovery discloses; grants
  permit; the two are separate fields end to end.
- The protocol version (`workcell.control/v1`) is the compatibility contract.
  Software revisions are provenance: reported, never refused on, never a
  trigger to update the other cell.
- Credential material is never recorded by any surface — only its SHA-256, on
  the serving side, and at most a keychain *reference* on the client side.
- Connection records carry no absolute paths and no generated runtime state:
  endpoint, protocol, identities, granted operations, credential reference,
  state, timestamps, provenance.

## §5 case → command mapping

### Case 1 — authorised remote use; execution location proved, not assumed

Host cell (its own state root, e.g. `WORKCELL_HOME=/home/me/wc-host`):

```bash
workcell --workcell-ref workcell:host authorise --client laptop \
    --allow status --allow discover --allow prepare
# note the printed credential
workcell serve --listen 0.0.0.0:7777
```

Client cell (its own state root):

```bash
workcell --state-root ~/wc-client connect --endpoint HOST:PORT \
    --connection laptop --authorization wck_...
# evidence (plain output):
#   "remote workcell: workcell:host"
#   "execution location: operations through this connection run on
#    workcell:host at HOST:PORT, not on this machine"
workcell --state-root ~/wc-client --endpoint HOST:PORT --authorization wck_... \
    --receipt world.json prepare --demand-ref demand:probe --require shell
# evidence: world.json carries "workcell_ref": "workcell:host" — the receipt
# names the cell that materialised the world; the local cell's status does not
# list it as local material.
```

### Case 2 — permitted disclosure; advertising is not authorisation

Evidence fields, same session as case 1:

```bash
workcell --state-root ~/wc-client connections show laptop --json
#   connection.granted_operations == exactly the granted list, nothing more
#   connection.advertised_offers is counted only because `discover` is granted
```

Negative probes:

- an operation outside the grant refuses loudly and names the grant:
  `... release` → "does not permit operation `release`";
- an unauthorised client learns only the refusal and the remedy:
  `connect` with an unknown credential → "no active connection grant matches",
  and the refused record carries no `remote_workcell_ref`.
- scoped advertisement: `authorise --advertise <port>` limits which ports
  discovery shows to that client.

### Case 3 — disconnect, offline state, host restart and reconciliation

```bash
workcell --state-root ~/wc-client connections disconnect laptop   # operator disconnect; record kept
# stop the host serve process; then:
workcell --state-root ~/wc-client connect --endpoint HOST:PORT ...
#   → "endpoint ... is unreachable"; record state `disconnected` (not refused)
# restart serve on the same endpoint; then reconnect:
workcell --state-root ~/wc-client connect --endpoint HOST:PORT ...
#   → "reconnected `laptop`"; record state `connected`; a reconnect rewrites
#   only the connection record — never a retained remote-world receipt.
```

### Case 4 — expired and revoked access; unsupported version refuses loudly

Revocation:

```bash
# on the host cell:
workcell --workcell-ref workcell:host revoke --client laptop
# at the client's next use (connect or any remote operation):
#   → refused with "revoked"; the client record keeps state `refused`
#     with the reason as detail. Audit evidence stays in grants.json.
```

Expiry:

```bash
# on the host cell:
workcell --workcell-ref workcell:host authorise --client laptop \
    --allow status --expires-in 30m
# the authorise output and the grants registry carry `expires_at_unix_ms`;
# the client's connection record carries the same instant after connect.
# past that instant, at the client's next use:
#   → refused with "expired" and the grant named — the same loud refusal
#     shape as revocation, but a different word: access ended on its own
#     terms, not by operator action. The record keeps state `refused` with
#     the reason as detail; the expired grant stays in grants.json.
# a grant authorised without --expires-in never expires.
```

Version skew: the protocol version is the contract. A client pointed at a
cell serving a different `workcell.control/vN` refuses loudly:

```
connection refused: remote control protocol `workcell.control/vN` is not
supported by this cell (`workcell.control/v1`); the two cells are incompatible
```

and the record keeps state `incompatible`. The honest way to prove this
between two real machines is a deliberately skewed build of the same source
(one constant changed); software-revision differences inside the same protocol
must NOT refuse — they are reported as a note ("nothing was updated").

### Case 5 — reconnect with compatibility reporting

Covered by the reconnect in case 3. Evidence (plain output):

```text
reconnected `laptop` -> HOST:PORT
compatibility: protocol workcell.control/v1 on both cells
software: local `workcell X (rev)` / remote `workcell Y (rev)`   # + note if they differ
```

### Case 6 — scoped changes, no copied identity between machines

The design keeps the two cells' state disjoint by construction: the grants
registry lives only in the serving cell's state root; connection receipts
live only in the client's. No machine identities, credentials, absolute paths
or generated runtime state are shared through the connection surface — the
records carry endpoint, identity refs and a credential *reference*, nothing
path-shaped. The connection-side share of the case is proven by inspecting
both state roots after the full run (cases 1–5, 7) for the absence of the
other cell's paths, credentials or generated state.

The shared-ground half of the case — a scoped source change flowing between
two machines that hold related ground, and a divergent change surfacing as an
explicit conflict — belongs to the composition lock's ground/source
synchronisation, which is Central's native source machinery, not Workcell's.
It is run through Central's source transfer Actions
(`projectcentral.source.transfer.export/apply/conflicts/resolve`; see Central
`docs/SOURCE-TRANSFER.md`):

1. **Related ground.** Both machines hold the same world (same project id),
   with the shared fork content placed on both before the receiving ground
   first reconciles: their shared lineage is shared *content revisions*,
   never copied `.central` state, machine identities or credentials.
2. **Scoped change, explicit direction.** The origin machine exports a
   bundle naming the exact source refs, the destination world, and
   operator-declared from/to ground labels — human-chosen names, the same
   discipline as a connection label above, because machine identities are
   not portable authored ground. The bundle carries world-relative paths and
   content revisions only; inspect it for the §5 prohibitions before moving
   it (no absolute paths, no credentials, no hostnames/usernames, no
   generated state).
3. **Transport.** The bundle moves as an ordinary file (`scp` or any file
   channel the operator already trusts). Workcell's connection is not the
   channel: its control plane is material and must not grow source
   semantics — the three synchronisations are never collapsed.
4. **Apply on the receiving ground.** Every entry either fast-forwards from
   its recorded base through the ordinary compare-and-swap write, is already
   present, or — when both grounds moved from the shared revision — records
   an explicit conflict naming base, incoming and local revisions, with both
   sides snapshotted and the source left byte-identical. Nothing is
   overwritten in either direction, and the receiving ground's own write
   authority governs every entry (human-authored ground refuses a declared
   non-human transfer).
5. **Resolution.** An explicit, recorded resolution closes the conflict:
   keep-local, or accept-incoming on the exact recorded local revision.

Evidence to capture mirrors the connection half: the bundle and both
grounds' `.central/source-transfer` records with digests, the byte-identical
conflicted source, the surfaced conflict record naming both revisions, and
the prohibition scans over the bundle and both grounds' records.

### Case 7 — independent client teardown; remote world intact

```bash
# on the client, after cases 1–5:
workcell --state-root ~/wc-client connections disconnect laptop
# on the host cell, nothing was removed: the worlds the host materialised for
# the client are still present and inspectable by the host:
workcell --workcell-ref workcell:host status --json
workcell --workcell-ref workcell:host instances
# hosted-service removal naming what connected clients lose is the host
# operator's explicit action (release/reconcile), never a side effect of a
# client disconnect.
```

## Secret projection rides the connection, never the payload

A connection is also how a credential reaches work on another cell without
leaving its origin store. The origin cell holds a `SecretProjectionRequest`
against the connected target (`workcell secret project …`, recorded in
`<state-root>/secrets/projections.json`), and the projection travels as an
authorised reference/materialisation relation under the same grant
discipline as everything else on this surface:

- capability advertisement is not authorisation, and neither is knowing a
  credential ref: material only ever moves through the approved sink
  boundary for the target's materialisation class — a sandbox's Credential
  Vault broker, or a use-time materialisation presented back to the origin
  over an authorised connection. A denied route performs zero
  provider-side writes;
- the control plane carries refs, classes, purpose and scope — never
  material. Receipts name the projection and the sink; they never contain
  the value;
- `workcell secret revoke-projection` marks the record revoked and keeps
  it as audit evidence. The projected target is refused at its next
  materialisation with a declared refusal naming the projection — the same
  "takes effect at the connecting client's next use" semantics as grant
  revocation;
- a declined origin or an unavailable source is a declared refusal with a
  reason, never a silent fallback to plaintext and never an empty result;
- `workcell secret scan` / `workcell secret vault` keep the origin honest
  about material that never entered a provider: detection reports location
  and presence only, and vaulting moves the value into the origin store
  without ever printing it.

## Where state lives

```text
serving cell   <state-root>/connections/grants.json    grant records (credential digests, optional expiries)
client cell    <state-root>/connections/<label>.json   connection receipts
origin cell    <state-root>/secrets/projections.json   secret projection records (refs only)
```

Both are ordinary durable state: inspectable, carried by the cell's own
lifecycle, never a hidden global.
