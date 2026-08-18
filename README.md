# EpiLogos Workcell

Workcell is the relatively standalone **technological materialisation product** beneath EpiLogos semantic clients.

Higher-level systems can say what an act means without knowing which container runtime, VM technology, host, network or storage provider will realise it. That abstraction is valuable, but it does not remove the material problem. At some point a semantic demand still has to become an actual computational world with a place, resources, processes, connectivity, persistence, lifecycle and observable consequences.

Workcell exists to own that transition.

It turns provider-neutral demand into a reachable, inspectable material world while keeping provider choices from leaking upward into the identity of Projects, Runs, Agents, Candidates or other semantic objects.

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

A reference Ubuntu worker, Docker, Arrakis, Tailscale, exe.dev or any future provider can be a serious proving specimen without becoming Workcell's ontology. The abstraction is successful when those technologies can change while the material demand remains intelligible.

## Control plane and native data plane

Workcell owns the control operations required to prepare and manage material worlds:

```text
discover
plan
prepare
observe
expose
collect
release
reconcile
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

The result should be **less infrastructure babysitting without loss of material intelligibility**.

## What changes for an agent

An agent can ask for a material capability through stable semantics rather than hard-coding host paths, IP addresses, Docker network names or provider-specific commands into its higher-level reasoning.

It can receive a structured description of the world that was prepared, operate through the bound native services and later return material evidence to the system that owns the purpose of the act.

## Relation to the wider {O:I} field

**O:I** is the whole technological-agency field. Workcell is its materialisation centre, not a mandatory substrate for every possible O:I arrangement.

**Central** can express durable machine roles and authored intent. Workcell owns the live material placement and observed state; observation does not rewrite Central's authored source automatically.

**Actuation** owns Agent/Agency identity, determination, authority and Return. Workcell can host the processes and services through which an Agency acts without defining who that Agency is.

**AIKit** resolves the operative semantic horizon — models, capabilities, sessions, runtime bodies, Surfaces and provider offers. Workcell answers the narrower physical question: how can this deployment make the required material conditions true?

**Software Factory** reasons in Projects, Runs, Candidates and evidence. Workcell can materialise several candidate worlds independently while Factory retains their developmental identity and reason for existence.

**Quaternal Logic** may analyse or formally refract material evidence where requested, but Workcell does not require QL semantics to prepare or observe a world.

## Current implementation

The product is implemented as a Rust workspace. The current core establishes the provider-neutral public domain seam, opaque client-reference boundary and material planning/lifecycle concepts. [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) is the current architecture authority for the complete Workcell territory.

The reference implementation and its tests are designed to prove the abstraction through real provider shapes rather than define the abstraction by one provider.

Current main and repository verification determine what is implemented now. Provider-specific reference work and future placement/distribution remain separate from the semantic contract until their evidence is accepted.

## Architecture and verification

Read:

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — product boundary, material contract and provider-neutral architecture.
- [`docs/MATERIALISATION-SPEC.md`](docs/MATERIALISATION-SPEC.md) — material world and binding semantics.
- [`docs/LIFECYCLE-AND-CANDIDATES.md`](docs/LIFECYCLE-AND-CANDIDATES.md) — lifecycle, reconciliation and Candidate materialisation.
- [`docs/CONNECTIVITY-FABRIC.md`](docs/CONNECTIVITY-FABRIC.md) — logical connectivity and fabric/provider separation.
- [`docs/CONTROL-SERVICE-AND-AGENT-HOSTING.md`](docs/CONTROL-SERVICE-AND-AGENT-HOSTING.md) — collapsed-local versus remote control and persistent service hosting.

Run the repository verification with:

```bash
./scripts/verify.sh
```

The same verification operation is used locally, by agents and by GitHub Actions.
