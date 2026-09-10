# Installed-world and second-placement procedure

Use the native operations in [CAW material operations](CAW-MATERIAL-OPERATIONS.md).
The [example packet](examples/caw-installed-campaign.json) is deliberately disabled:
false authorisation, placeholders and empty restart commands prevent execution.
It is not an installed configuration or evidence of a second host.

On each approved host, after its exact-source installation has been separately
authorised and performed, record a read-only census:

```bash
python3 scripts/caw_installed_world.py census \
  --binary /absolute/path/to/workcell \
  --state-root /absolute/existing/material-test-state \
  --repo /absolute/path/to/Workcell \
  --output /absolute/private/new-census.json
```

The census collects binary hash/version, source SHA/dirty standing, OS/kernel,
host identity hash and declaration digests. It never installs, chmods, copies
Control/NOW, changes governance or uploads private evidence. A supplied census is
not remote attestation: independently verify endpoint-to-host binding locally.

Complete a private copy of the example with two genuinely distinct host censuses,
exact authorised Workcell refs/endpoints, credential environment-variable names,
actual owner service/NOW requirements, retained semantic correlations and exact
native supervisor restart argv. Use authorised test Worlds, not arbitrary existing
personal sessions. Set authorisation only after approval. Both sites are preflighted
before any prepare; incomplete restart or release choices are refused.

```bash
python3 scripts/caw_installed_world.py exercise \
  --manifest /absolute/private/approved-packet.json \
  --execute-authorized --output /absolute/private/new-campaign.json
```

The procedure reserves a new private evidence file before effects. Each material
phase and returned receipt is fsynced into a new append-only `.journal.jsonl`
sidecar, so an interrupted harness leaves the last known operation instead of
silently losing partial effects. Neither file is overwritten by a new campaign.
After forced termination, inspect that journal and actual native material state;
never delete it merely to award a successful retry.

The procedure performs prepare, inspect, observe, explicit native restart,
reconnect, recover and the explicitly chosen release/retain disposition at both
placements. Empty demands and empty observations cannot count as hosting proof.
It does not transfer source bytes or credentials, migrate NOW, instantiate an
AIKit implementation, or attest successful Factory work. Those local owner joins,
installed write-boundary coverage, independent second-placement verification and
whole-operation/human acceptance remain separate proof cases.

Procedure safety tests use controlled inputs and explicit mocks solely to check
refusal and evidence preservation; they are not two-host execution evidence:

```bash
PYTHONPATH=scripts python3 -m unittest -v scripts/test_caw_installed_world.py
```
