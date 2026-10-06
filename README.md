# Workcell

Workcell is a Rust command-line tool and control service, `workcell`. It turns a request for a place to work, such as a workspace at a given revision, a shell or a set of services, into a real workspace, process or service, on this machine or on a connected one. It records what was actually provided, where, and what became of it. Workcell holds O:I's **environment** facet: the workspaces, services and machines where work runs.

## What it does today

The unit of work is an **execution demand**. It is a provider-neutral request that says what an act needs, sorted into what is required, what is preferred and what is optional. A demand might ask for a writable checkout at a revision, a shell and internet access, with snapshot support preferred and a GPU optional. Workcell:

1. **discovers** what this machine can actually offer (`workcell discover`);
2. **plans** a match, and states every preference it had to drop (`workcell plan`);
3. **prepares** the world and writes a receipt (`workcell prepare`);
4. **observes, exposes and collects** from that world (`observe`, `expose`, `collect`);
5. **releases or preserves** it at the end (`release`), and can **recover** a prepared world from its receipt after a restart without inventing a new identity (`recover`, `reconcile`).

What was provided is kept as a **binding graph**: each requirement is tied to the provider, the offer and the concrete resource that satisfied it, with health, provenance and a presence state. The presence states are present, missing, released, suspended, snapshotted and stale. A world whose ephemeral material has disappeared is reported as lost, not quietly recreated.

**Works now in the shipped binary:**

- **Zero-daemon local mode.** Directory and git-worktree workspaces, host-process execution, directory artifact storage, a run ledger (`workcell run start|observe|collect|release|list|show`) and receipts.
- **Services.** Services are declared in `services.json`. Workcell observes services that something else started ("target-owned") and starts and stops child-process services itself ("managed").
- **Inspection.** `workcell status`, `providers`, `inspect` and `doctor` (`doctor` checks the zero-setup local baseline).
- **Process census.** `workcell places` is a read-only census of tmux and herdr panes as persistent places for processes; `place request|release` acts on them. `workcell instances` lists live agent-harness processes, using Actuation's harness detection, and gives bounded CPU and memory readings that never read process arguments or environment.
- **Cross-machine connections.** `serve`, `authorise`, `connect`, `revoke`, `connections` and `machine add|list|remove` run one Workcell's commands against another over `workcell.control/v1`. Each client gets a named grant of operations and a credential shown once. A grant can expire, and an expired grant is refused as expired, distinct from revoked.
- **Secrets by reference.** `secret scan` reports where credential material sits outside a secret store, giving the location only and never the value. `secret vault` moves it into the macOS Keychain, 1Password or the Linux Secret Service. `secret project` records an authorised projection by reference. Delivery works today only to an OpenSandbox allocation, through its credential broker.
- **Sandboxes.** An OpenSandbox sandbox rung (`run --rung sandbox`, `sandboxes reconcile`) works when an OpenSandbox server is declared.
- **Disclosure to O:I.** `workcell system` and `workcell config-contribution` emit O:I's settings and configuration contracts.

**Built as libraries, not yet in the shipped binary:** the Docker Engine and Compose adapter, the Arrakis MicroVM adapter (its live gate is not yet closed), the Tailscale fabric provider, SMB file-share staging, multi-Workcell placement, and Factory interop conformance.

**Not implemented:**

- There is no VM provider and no database provider; a database can only be a declared service.
- No network-fabric provider is registered in the binary.
- GPU and host-hardware enumeration are reported as unavailable.
- Projecting a secret to another Workcell is refused, even though `--help` lists it.
- Workcell does not create machines. `machine add` declares an existing machine that already runs Workcell. A separate script, `scripts/exe-dev-workcell-bootstrap.sh`, can bootstrap one on exe.dev.

## How it fits O:I

O:I gives each facet of an agent's world its own product. Workcell holds **environment**: the concrete place where an act runs. It owns how a requirement becomes real here. It does not own why the requirement exists, or the identity of the Project, Run, Agent or session that uses the place. Callers pass those in as opaque references.

| Neighbouring facet | Where they meet |
|---|---|
| Agency / Actuation | Before a run, Workcell asks Actuation whether the selected Agency may act (`actuation agency actualise`, `actuation.agency-actualisation/v1`). Actuation decides authority, and Workcell checks only that the answer covers the exact request. The instance census reads `actuation.harness-detection/v1`. |
| Development / Software Factory | A run may carry an optional Factory run reference; a run without Factory is first class, and Workcell never invents Factory ancestry. Resource readings carry opaque correlation references that Factory can fill. `workcell-interop` checks the `factory.interop/v1` fixtures. |
| Capability / AIKit | `correlate-projection` attaches AIKit's worktree-projection verdict (`aikit.worktree-projection/v1`) to a world's checkout, carried verbatim. Workcell also hosts a reference AIKit Agency Gateway service. |
| Ground / Central | Credential references use Central's `central.security/v1` scheme. Re-placing an agent instance on another machine needs Central's machine binding (`Control/machines/current.json`). Observation never rewrites Central's authored source. |
| Reflection / QL | No code connection. QL may read material evidence if asked; Workcell does not need QL. |

**In the Cradle.** The O:I desktop treats the material body as mostly implicit: "this Mac" by default, with named machines, services and placement shown when they matter. The Cradle's kernel reads Workcell's material and service state as one of its owner readings.

Central's `seat` instrument, which manages the env-1/2/3 development seats, is a separate Central tool and does not call this product.

## Install and quick start

The builds are pre-releases for Apple Silicon macOS and x64 Linux. `oi` reports that they "have not passed physical acceptance".

Through O:I (ordinary route):

```sh
oi install workcell
workcell status
```

From source (developer route; stable Rust with clippy and rustfmt):

```sh
git clone https://github.com/EpiLogos/Workcell
cd Workcell
cargo install --path crates/workcell-cli --locked   # installs workcell, workcell-control-service, workcell-control-client, workcell-write-boundary
workcell doctor
```

Release archives with SHA-256 sidecars are on the releases page (`workcell-v0.1.0-prelocal.6`).

First commands:

```sh
workcell status                        # this Workcell, its providers, offers and connections
workcell discover                      # what this machine can provide right now
workcell plan --require shell --workspace writable --json   # plan only; nothing is created
workcell places                        # read-only census of tmux / herdr places
workcell instances list                # live agent-harness processes
workcell secret scan                   # where credentials sit outside a store (presence only)
```

State lives in `~/.workcell`, or in `$WORKCELL_HOME` if that is set. Without `--endpoint`, `workcell` runs entirely locally with no daemon. With `--endpoint HOST:PORT`, or `WORKCELL_CONTROL_ENDPOINT`, the same commands run against a remote Workcell.

Per-command `--help` prints the combined help. The full help text is the CLI reference.

## Verify

```bash
./scripts/verify.sh
```

The same verification operation is used locally, by agents and by GitHub Actions. It runs `cargo fmt --check`, `cargo clippy -D warnings` and `cargo test` over the workspace. Live Docker and Arrakis smokes are opt-in (`WORKCELL_DOCKER_LIVE=1`, `WORKCELL_ARRAKIS_LIVE=1`).

---

## More than "execution"

Calling Workcell an execution layer is too narrow.

An act may require much more than starting a process. It may require:

- a writable workspace at a particular source revision;
- a long-lived service;
- a container, MicroVM, VM or host process;
- a remote machine;
- a project runtime with several services;
- a database or other persistent state;
- logical network relationships;
- credentials and artifact channels;
- a browser-accessible Candidate;
- observation, recovery, retention and eventual release.

These requirements together form the **material conditions in which the act can actually occur**. Workcell makes those conditions explicit and returns enough observed state and evidence for higher-level systems to know which world was really inhabited.

*(Design scope. For which of these the shipped binary provides today, see "What it does today" above.)*

## The central relation

```text
semantic demand
      ↓
provider-neutral material requirements
      ↓
Workcell
      ↓
provider matching + bindings
      ↓
MaterialisedExecutionWorld
      ↓
workspace · process · service · container · VM · host
storage · network · database · browser surface · other provider form
      ↓
observed state + artifacts + material evidence
      ↑
semantic client
```

The upper system owns **why** the world is needed. Workcell owns **how that requirement becomes material here**.

This separation matters because provider-neutral semantics are only genuinely portable if they survive contact with real provider differences. A Project should not become "a Docker project" merely because Docker happened to satisfy today's demand. A Candidate should not change identity because its runtime moves from a local container to a remote VM. An Agent should not become a different Agent because its process was rematerialised elsewhere.

## Material placement is part of provenance

Material implementation is not semantically authoritative, but it can be evidentially important.

A later human or agent may need to know:

- which source/workspace was actually mounted;
- which provider satisfied isolation;
- what services were reachable;
- whether public internet was available;
- what storage persisted;
- which endpoint exposed a Candidate;
- what process or service was healthy;
- what was released, retained or recovered.

Workcell therefore preserves a `BindingGraph` and observed material state rather than returning only "execution succeeded".

That evidence can explain why two otherwise similar acts differed without turning the physical provider into the semantic identity of the work.

## Provider neutrality has to survive reality

A semantic client expresses requirements and preferences:

```text
required
    writable project source
    shell
    internet

preferred
    strong isolation
    snapshot / rollback
    browser exposure

optional
    GPU
```

Workcell advertises what a deployment can actually provide, plans a match, makes degradation explicit and materialises the chosen bindings.

A reference Ubuntu worker, Docker, Arrakis, Tailscale, an exe.dev remote bootstrap or any future provider can be a serious proving specimen without becoming Workcell's ontology. The abstraction is successful when those technologies can change while the material demand remains intelligible.

## Control plane and native data plane

Workcell owns the control operations required to prepare and manage material worlds:

```text
discover
plan
prepare
inspect
observe
expose
collect
release
reconcile
recover
```

Once a binding exists, ordinary data should generally flow through the native protocol between the workload and the bound service. Workcell does not become a universal proxy merely because it created the relation.

```text
Factory / AIKit / other client
        ↓ allocate · resolve · observe
     Workcell
        ↓ bindings
      workload ─────────→ project API
               ─────────→ database
               ─────────→ search service
               ─────────→ internet
```

The control plane answers how the world is made and maintained. The data plane remains native.

## What changes for a human

A human can reason in terms of the thing they are trying to make real — a Candidate, Project runtime, isolated task or persistent service — without having to manually reconstruct the provider topology each time.

At the same time, the material world is not hidden behind a magical "run" button. Plans, bindings, lifecycle, degradation and observed state remain inspectable when consequence or debugging requires them.

## What changes for an agent

An agent can ask for a material capability through stable semantics rather than hard-coding host paths, IP addresses, Docker network names or provider-specific commands into its higher-level reasoning.

It can receive a structured description of the world that was prepared, operate through the bound native services and later return material evidence to the system that owns the purpose of the act.

## Relation to the wider O:I field

**O:I** is the whole technological-agency field. Workcell is its materialisation centre, not a mandatory substrate for every possible O:I arrangement.

**Central** can express durable machine roles and authored intent. Workcell owns the live material placement and observed state; observation does not rewrite Central's authored source automatically.

**Actuation** owns Agent/Agency identity, determination, authority and Return. Workcell can host the processes and services through which an Agency acts without defining who that Agency is.

**AIKit** resolves the operative semantic horizon — models, capabilities, sessions, runtime bodies, Surfaces and provider offers. Workcell answers the narrower physical question: how can this deployment make the required material conditions true?

**Software Factory** reasons in Projects, Runs, Candidates and evidence. Workcell can materialise several candidate worlds independently while Factory retains their developmental identity and reason for existence.

**Quaternal Logic** may analyse or formally refract material evidence where requested, but Workcell does not require QL semantics to prepare or observe a world.

## Repository layout

| Crate | What it holds |
|---|---|
| `workcell-core` | provider-neutral contracts: demand, planner, offers, provider ports, binding graph |
| `workcell-runtime` | the collapsed-local Workcell: host processes, services, storage, run ledger, harness census, places, resource usage |
| `workcell-cli` | the `workcell` binary and the control-service, control-client and write-boundary binaries |
| `workcell-control`, `workcell-wire` | the `workcell.control/v1` service and client, grants, machine registry, secret-projection ledger; JSON wire formats |
| `workcell-workspace`, `workcell-artifact` | directory and git-worktree workspaces; artifact storage |
| `workcell-sdk` | the stable facade for provider authors |
| `workcell-opensandbox`, `workcell-docker`, `workcell-arrakis` | execution providers (only OpenSandbox ships in the binary) |
| `workcell-fabric`, `workcell-tailscale` | network-fabric types and the Tailscale provider (library) |
| `workcell-keychain`, `workcell-onepassword`, `workcell-secret-service`, `workcell-secret-scan` | secret stores and the credential scan |
| `workcell-fileshare`, `workcell-candidate`, `workcell-placement`, `workcell-interop` | SMB staging, Candidate view, multi-Workcell placement, Factory interop conformance (libraries) |

## Documentation

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — product boundary, material contract and provider-neutral architecture; the current architecture authority for the complete Workcell territory.
- [`docs/ARCHITECTURE-NAVIGATION.md`](docs/ARCHITECTURE-NAVIGATION.md) — each native operation and the source file that implements it.
- [`docs/LIFECYCLE-RECONCILIATION.md`](docs/LIFECYCLE-RECONCILIATION.md) — binding presence, restart recovery and conservative reconciliation.
- [`docs/CANDIDATE-MATERIALISATION.md`](docs/CANDIDATE-MATERIALISATION.md) — materialising a Factory-owned Candidate, possibly many times.
- [`docs/CROSS-CELL-CONNECTIONS.md`](docs/CROSS-CELL-CONNECTIONS.md) — serve / authorise / connect / revoke and the grant rules.
- [`docs/PROVIDER-SDK.md`](docs/PROVIDER-SDK.md) — writing a provider against `epilogos-workcell-sdk`.
- [`docs/RESOURCE-USAGE.md`](docs/RESOURCE-USAGE.md) — bounded, truthful CPU and memory readings for live HarnessInstance processes.
- [`docs/PLACE-CENSUS.md`](docs/PLACE-CENSUS.md) — the tmux / herdr place census.
- [`docs/CONNECTIVITY-FABRIC.md`](docs/CONNECTIVITY-FABRIC.md) — logical connectivity and fabric/provider separation.
- [`docs/CONTROL-SERVICE-AND-AGENT-HOSTING.md`](docs/CONTROL-SERVICE-AND-AGENT-HOSTING.md) — collapsed-local versus remote control and persistent service hosting.

---

## Background

O:I stands for Objective : Internality. It names the means through which a life knows and acts within a world: memory, language, tools, permissions and other people. Those means are internal because every act proceeds through them, and objective because each can be examined and changed. Workcell makes one of those means, the material place where an act runs, something that can be requested, inspected and released. The idea is developed in the essay [*Confronting the Limit: Determination, Subjectivity and Mind as Objective Internality*](https://oi.epi-logos.org/essay/).
