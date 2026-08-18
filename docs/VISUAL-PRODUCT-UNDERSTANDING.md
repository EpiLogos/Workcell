# Workcell Visual Product Understanding

**Status:** canonical product-understanding surface  
**Architecture status:** accepted `main`, including provider-neutral core, Docker/Arrakis/runtime providers, placement/control/SDK seams, model-serving conformance and material authority/secret boundaries  
**Sources:** `ARCHITECTURE.md`, `CANDIDATE-MATERIALISATION.md`, `LIFECYCLE-RECONCILIATION.md`, provider integration docs, current Rust crates, and accepted conformance fixtures.

Workcell exists at the point where semantic demand must stop being merely a description and become an inspectable computational body. Its abstraction is valuable only if that boundary remains visible.

## 1. Experience — a demanded capability becomes an actual world you can inspect

```mermaid
flowchart TB
    NEED["Something needs a real computational world<br/>run · candidate · service · agent body · deterministic operation"]
    ASK["Describe what must be materially true<br/>without naming a provider"]
    WORLD["A reachable executable world exists"]
    USE["The workload can operate in it"]
    KNOW["Later we can inspect what actually existed<br/>bindings · health · endpoints · resources · lifecycle evidence"]

    NEED -->|"becomes material requirements"| ASK
    ASK -->|"Workcell plans and prepares"| WORLD
    WORLD -->|"supplies concrete reachability and resources"| USE
    USE -->|"produces observations and artifacts"| KNOW
```

The user or higher-level agent reasons in terms of the world required, not Docker bridge names, VM IDs, fixed host paths or provider brands. Those details remain available as provenance after materialisation.

## 2. Product / conceptual relation — abstraction crosses into material actuality

```mermaid
flowchart TB
    SD["Semantic / material demand<br/>required · preferred · optional affordances"]

    subgraph Boundary["Provider-neutral Workcell boundary"]
      WC["Workcell contract<br/>discover · plan · prepare · observe · expose · collect · release · reconcile"]
      PLAN["Binding / placement plan<br/>requirements matched to current offers"]
      WC -->|"resolves requirements into"| PLAN
    end

    PB["Provider binding / placement<br/>workspace · execution · runtime · service · storage · fabric"]
    BODY["Real computational body<br/>process · container · MicroVM · service · network · storage · endpoint"]
    OP["Operation and lifecycle"]
    EV["Evidence of what actually existed<br/>BindingGraph · observed state · health · outputs · resource provenance"]

    SD -->|"asks what must be true, not how"| WC
    PLAN -->|"selects concrete means"| PB
    PB -->|"materialises"| BODY
    BODY -->|"supports"| OP
    OP -->|"is observed as"| EV
    EV -. "returns material reality to the semantic client" .-> SD
```

The horizontal cut is the point: **semantic identity stays above; bindings and physical identities live below**. A Candidate, Agent, Run or Harness may use a Workcell world without being redefined as that container, VM, process, endpoint or host.

## 3. Architecture — current Rust materialisation stack

```mermaid
flowchart TB
    CLIENT["Semantic clients<br/>opaque Project / Run / Agent / Candidate refs"]
    CORE["workcell-core<br/>ExecutionDemand · OperationalOffer · planning · BindingGraph / world contracts"]
    PLACE["workcell-placement<br/>placement and multi-Workcell selection"]
    CONTROL["workcell-control + workcell-sdk + workcell-wire<br/>local or remote control-plane access"]

    subgraph Providers["Provider implementations"]
      WS["workcell-workspace"]
      DK["workcell-docker"]
      AR["workcell-arrakis"]
      RT["workcell-runtime"]
      ART["workcell-artifact / candidate support"]
    end

    PHYS["Native material world<br/>workspaces · processes · containers · MicroVMs · model services · storage · networks · endpoints"]
    OBS["observe / expose / collect / release / reconcile<br/>material receipts and evidence"]

    CLIENT -->|"submits provider-neutral demand"| CORE
    CONTROL -->|"carries the same control contract"| CORE
    CORE -->|"may choose location through"| PLACE
    CORE -->|"binds requirements to provider ports"| WS
    CORE -->|"binds requirements to provider ports"| DK
    CORE -->|"binds requirements to provider ports"| AR
    CORE -->|"binds requirements to provider ports"| RT
    CORE -->|"binds artifact/candidate material channels"| ART

    WS --> PHYS
    DK --> PHYS
    AR --> PHYS
    RT --> PHYS
    ART --> PHYS
    PHYS -->|"is inspected rather than assumed"| OBS
    OBS -->|"returns material truth with provenance"| CLIENT
```

Provider-specific IDs, paths, ports, engine flags and lease/material-secret details are binding/provenance facts. The current containment and secret-materialisation work strengthens the **physical authority boundary**; it does not move semantic Action/Agency/Project authority into Workcell.

## 4. Diagram audit

| Existing visual | Class | Disposition |
|---|---|---|
| `ARCHITECTURE.md` semantic client → ExecutionDemand → core → provider ports → BindingGraph → physical resources | architecture | **Preserve but supersede as the only first visual.** It is accurate; the new experience and conceptual diagrams explain why that stack exists and make the abstraction/material cut clearer. |
| collapsed-local vs remote Control Service ASCII | specialist architecture | **Preserve.** It explains transport topology without redefining the Workcell contract. |
| connectivity/fabric diagrams | specialist architecture | **Preserve.** They explain logical reachability and provider realisation. |
| Candidate materialisation diagrams | cross-product specialist | **Preserve.** Candidate remains Factory-owned while Workcell supplies the body. |
| deployment-profile and multi-Workcell maps | operational architecture | **Preserve.** They answer placement questions after the provider-neutral relation is understood. |
| provider-specific Docker/Arrakis/model-serving diagrams and fixtures | implementation/evidence | **Preserve.** They are conformance specimens, not the ontology. |

## 5. Verification

**Semantic:** the conceptual diagram makes the abstraction/material boundary spatially explicit. The arrows distinguish requirement, resolution, binding, materialisation, operation, observation and return.

**Implementation:** the architecture uses crate and operation seams present on accepted `main`. It does not claim a cluster framework, universal daemon, specific fabric brand or one mandatory isolation technology.

**Cross-product:** Workcell is not AIKit: AIKit resolves what operative world/body should be available, while Workcell materialises material requirements. Workcell is not a generic executor: the durable product is the provider-neutral relation plus inspectable bindings/lifecycle evidence, not simply “run command”.

## 6. Public-site projection

Project the **abstraction crossing into a real body** diagram for the public/design surface. A deeper technical page can reinterpret the Rust provider architecture. Do not lead with Docker, Arrakis, ports or deployment profiles; those are proof that the contract survives heterogeneous bodies, not the human reason for the product.