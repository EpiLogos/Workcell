# Session prompt — Workcell secret origin & cross-machine projection

Paste this as the opening prompt of a fresh session. Work happens in
`/home/frank/Work/Workcell` (this repo). Keep secrets out of all output,
receipts and diagnostics — refs and presence only, never material.

---

## Context (read before acting)

- Workcell already owns a `SecretProvider` contract
  (`crates/workcell-core/src/secret.rs`), a macOS Keychain source
  (`crates/workcell-keychain`, `keychain://<service>/<account>` refs,
  read-only `resolve`, store/remove deliberately off the trait), a 1Password
  source, and the OpenSandbox Credential Vault broker
  (`crates/workcell-opensandbox`: use-without-read, egress-sidecar injection,
  denied-route → zero provider writes, `reinjection_required_after_sidecar_recreation`).
- The gap this session closes is **not** a new secrets ontology. It is:
  1. **One origin.** A secret is stored once, on the machine where the person
     put it (main Workcell). No second store, no per-machine copies, no split
     state between workcells.
  2. **Projection, not replication.** Other machines (workcell→workcell,
     workcell→OpenSandbox sandbox) receive the secret as an authorised
     reference/materialisation request. Raw material crosses only the
     existing trusted sink boundary; receipts carry refs, never values.
  3. **Linux parity at the source.** `workcell-keychain` refuses on Linux by
     design ("could not run" must never collapse into "ran and found
     nothing"). Add a Linux Secret Service (`org.freedesktop.secrets`)
     SecretProvider under the same contract and boundaries — this machine
     (Omarchy/GNOME keyring) is the proving host.
  4. **Exposure discovery.** The systems currently do not see unprotected
     keys. Example on this machine: a provider API key sitting in
     `~/.pi/agent/auth.json` (plaintext file, mode 0600). Add a
     `workcell secret scan` (or equivalent leaf command) that *detects*
     credential material in env vars, shell rc files and known auth files,
     reports location + presence only, and offers one-command vaulting into
     the origin store with the plaintext never echoed. Detection failure is
     declared, never silent.

## Non-negotiable boundaries

- Consumers resolve; they never write the vault (existing trait law).
- `resolve` reads only; materialisation classes, receipts and redaction stay
  in workcell-runtime; redacted `Debug` everywhere.
- Use-without-read holds for sandbox workloads: agent may *use* a credential,
  never *read* it.
- Cross-machine transport uses the existing cross-cell connection
  authorisation (`workcell authorise` / `workcell connect`, `wck_…`
  credentials, `--store-credential`), no new ad-hoc channel.
- A declined/unavailable source is a declared refusal with a reason, never an
  empty result or silent fallback to plaintext.

## Track A — implementation (on this Omarchy machine)

1. Read the `SecretProvider` trait, `workcell-keychain` and
   `workcell-onepassword` as the pattern; implement
   `secret-provider:secret-service/linux` (`linux-secret-service://` refs)
   with the same two boundaries and ACL-at-store-time story.
2. Add the projection operation: an origin workcell holds a
   `SecretProjectionRequest` (source ref, target workcell/sandbox relation,
   authorised materialisation class); the target receives via the existing
   broker/consumer seam. Extend `MULTI-WORKCELL-PLACEMENT.md` and
   `CROSS-CELL-CONNECTIONS.md` rather than minting new semantics.
3. Add exposure scan + vaulting flow per (4) above.
4. Tests: denied-route zero-write parity for projection; revocation reaching
   the projected target; scan finds a seeded dummy key in a temp rc file and
   reports it without printing it; Linux-source refusal text never fakes
   success on hosts without Secret Service.

## Track B — proving: OpenSandbox VM on Omarchy, driven from the Mac

Set up an OpenSandbox VM on this Omarchy machine and connect to it from the
Mac, so Workcell runs in its own space with a real cross-machine relation:

1. Read `docs/OPENSANDBOX-SOURCE-INTEGRATION.md` and
   `docs/DEPLOYMENT-PROFILES.md`; stand up the VM/sandbox worker here.
2. Join it via the connectivity fabric (Tailscale/existing fabric — no
   fixture servers, no credential bypasses).
3. From the Mac: connect, authorise, materialise a real credential from the
   origin store into the sandbox through Credential Vault, and prove
   use-without-read (workload authenticates; raw value appears nowhere,
   including on the Mac).
4. Then revoke at the origin and verify the projected target loses access.

## Track C — desktop UX: remote machines as a simple thing

The desktop app must make remote environments feel like what harness users
already expect: a simple remote-connection setup, after which the remote
machine's folders appear in the projects list for files and chats.

1. Walk the current desktop remote/connection surface (if any); name the
   first relation that breaks the story "add a machine, see its projects".
2. The connection flow must consume the same cross-cell authorisation
   (`wck_…` credential, stored via the origin keychain — not pasted into
   desktop config) and the same secret-reference law. No second connection
   truth, no plaintext in desktop state.
3. Write the walk up as a story-grade description (actor → operation →
   observable outcome → negation) against
   `Central/Work/O-I/docs/experience/STORIES.md` **without** fragmenting it
   across install variants: journeys are global; modality is a parameter of
   the walk, not a family of its own.
4. Do not build new desktop UI this session unless a native seam already
   exists to carry it; otherwise return the contract gap as an explicit
   issue.

## Definition of done

- A key stored once on the origin machine is usable by a workload in the
  OpenSandbox VM driven from the Mac, with zero raw-material transit and
  working revocation — demonstrated live, receipts clean of material.
- `secret scan` finds the seeded/real unprotected keys on this machine
  (including the one in `~/.pi/agent/auth.json`) and vaulting moves it into
  the origin store without the value ever being printed.
- Linux Secret Service provider passes the same test suite as the macOS
  keychain crate, with declared refusal where unavailable.
- No split state: grep the session's own diff for any new persistent secret
  store — there must be none; origin + refs only.
