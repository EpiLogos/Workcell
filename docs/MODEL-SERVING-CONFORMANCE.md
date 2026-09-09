# Model-serving materialisation conformance

**Owning ticket:** #29 (`W25 — Model-serving materialisation conformance`)  
**SDK boundary:** #23 / `epilogos-workcell-sdk`  
**Status:** deterministic lifecycle implementation plus opt-in physical-provider gates

This programme treats Ollama, llama.cpp and vLLM as pressure on ordinary Workcell material contracts. It does not add model, agent, harness, session or actuation ontology to Workcell.

The caller continues to own model/variant identity. Workcell owns only the material facts required to make execution possible: process/service lifecycle, artifact/storage placement, resource/accelerator capacity, endpoint/reachability, placement, provider health, provenance and reconciliation.

## Source pins

The fixtures in `crates/workcell-runtime/tests/model_serving_conformance.rs` are pinned to the upstream heads re-inspected on 2026-08-17:

| Provider | Upstream revision | Material behaviour used |
|---|---|---|
| Ollama | `48cb7b94e446bb3f32555d8e21a5552ebe463711` | `ollama serve` as the managed service process; `OLLAMA_HOST` as its bind address; CLI/API model acquisition/control remains separate from inference reachability. |
| llama.cpp | `ce8d842306b6e206f2833e04d472cff79c3c9be1` | `llama-cli` as a direct one-shot process and `llama-server` as a long-running HTTP service. |
| vLLM | `a0a3c32dd705fd447488262c757ffa18ab9e39d3` | `vllm serve <provider-model-id>` as a long-running service; accelerator capacity remains an ordinary Workcell resource requirement. |

Provider-native model identifiers, filesystem paths, arguments, bind addresses, process ids and revisions are recorded as provider/material properties or provenance. They never replace caller-owned semantic refs.

## New generic material seam

`ManagedHostServiceProvider` implements the existing public `ServiceProvider` port for an ordinary long-running host process. It is intentionally not model-specific.

A `ManagedHostService` supplies:

- a caller-facing logical service ref;
- a material endpoint;
- executable, arguments, environment and optional working directory;
- provider-native metadata/provenance;
- an optional TCP readiness probe.

The provider:

1. advertises the service only when the executable is materially available;
2. starts the process without shell interpolation;
3. waits for declared TCP readiness before returning a healthy binding;
4. records process id, endpoint, executable and metadata as material facts;
5. distinguishes a live but unreachable process from a healthy reachable service;
6. reports process disappearance as unavailable health;
7. releases by terminating and reaping the process;
8. refuses suspend/snapshot because a generic host process cannot honestly promise those lifecycle operations;
9. kills outstanding managed children when the provider itself disappears.

The same provider is suitable for any ordinary host service and is therefore a reusable Workcell-owned seam exposed by model-serving implementation evidence rather than a provider-brand abstraction.

## Reaching the seam from a real machine

Implementing the port is not the same as offering it. Until the collapsed-local
composition registered a `ServiceProvider`, `ManagedHostServiceProvider` existed
only for callers who linked `epilogos-workcell-runtime` and wired it themselves:
`workcell plan --connect inference:<caller-owned>` on an ordinary machine
answered `no offer supports this material requirement`, because the machine had
a workspace port, an execution port and an artifact port and no service port at
all.

`CollapsedLocalWorkcell` now always registers two service providers, so the
shipped `workcell` binary and `workcell-control-service` both have the port.
What they offer through it is whatever an operator has declared, in a JSON file
read from `<state-root>/services.json` or from `--services PATH`:

```json
{
  "schema": "workcell.service-declaration/v1",
  "services": [
    {
      "logical_ref": "inference:caller-owned-service",
      "lifetime": "provider-process-scoped",
      "endpoint": "http://127.0.0.1:21434",
      "program": "ollama",
      "args": ["serve"],
      "env": {"OLLAMA_HOST": "127.0.0.1:21434"},
      "readiness": {"host": "127.0.0.1", "port": 21434, "timeout_ms": 20000}
    }
  ]
}
```

The declaration carries no model, engine or vendor meaning into Workcell. The
logical ref stays caller-owned; executable, arguments, endpoint and readiness are
material facts the operator already knows. Workcell records them as provider
properties and provenance, exactly as the runtime tests already do.

A declaration is a claim about intent, not evidence of acceptance. The provider
still has to find the executable, start it and reach the endpoint. A declared
service whose program is not on the machine is offered as `unavailable`, and a
plan for it fails with `matching offers are unavailable` — which is a different
and more informative answer than the `no offer supports this material
requirement` returned when the port was missing entirely.

### Lifetime is part of the binding, not an implementation detail

`lifetime` is required in every declaration because the two answers behave
differently and a receipt that hid the difference would lie:

| `lifetime` | provider | who owns the process | survives the Workcell process |
|---|---|---|---|
| `provider-process-scoped` | `ManagedHostServiceProvider` | Workcell starts it as a child | no — reaped when the provider drops |
| `target-owned` | `ExternalManagedServiceProvider` | something else starts and supervises it | yes — re-observed through its own status command |

The distinction is not cosmetic. In the long-running Control Service a
provider-process-scoped child lives as long as the daemon and is released and
reaped on `release`. In a one-shot `workcell prepare` the same child dies when
the command returns, so the binding records `lifetime: provider-process-scoped`
in its properties and provenance and a later `workcell observe` reports it as
unavailable rather than claiming a service that is no longer there.

A `target-owned` service is re-enterable across invocations: a later process
rebuilds the record from the binding — refusing to do so unless the binding names
a service still declared at the same endpoint — and then lets the target's own
status command answer. `started_by_provider` is read from the binding rather than
assumed, so release never stops something Workcell did not start.

## Inference access versus control

The service binding proves only that an endpoint exists and is reachable.

It does **not** grant or imply model-control authority.

Provider-native control remains an explicit operation through existing Workcell process execution where the caller is authorised to request it. For example, Ollama model acquisition/unload/inspection can be represented by explicit `ollama pull`, `ollama stop` and `ollama ps` process operations. llama.cpp direct execution is likewise an ordinary `HostProcessExecutionProvider` operation. The endpoint and the control operation are separate material capabilities and may be exposed under different policy.

## Deterministic conformance

Standard repository verification exercises:

- real managed process start;
- TCP readiness and endpoint observation;
- PID/material provenance;
- release and process reaping;
- process disappearance -> unavailable observation;
- replacement/rematerialisation with stable logical identity but changed provider/material identity;
- executable/provider disappearance represented as unavailable rather than fake health;
- llama.cpp direct-process shape alongside server form;
- vLLM resource planning failing without accelerator capacity;
- the same vLLM demand becoming satisfiable when a separate remote execution offer provides the required accelerator capacity;
- upstream provider revisions retained as material provenance.

`crates/workcell-cli/tests/service_connectivity.rs` exercises the same contract
through the programs Workcell ships rather than through a linked library:

- `workcell plan` and `workcell prepare` satisfying a `connectivity:` demand from
  a declared service, starting the process and reaching its endpoint;
- the receipt disclosing `lifetime` so a provider-owned child is not mistaken for
  a service that outlives the command;
- a declared service whose executable is absent staying unsatisfiable, with the
  omission saying the offer was unavailable;
- a `target-owned` service re-entered and re-observed by a later, separate
  invocation through its own status command;
- the Control Service composition answering the same demand for a remote
  `workcell --endpoint` client.

The declared service in those tests is the test binary re-invoked as a socket
listener. It proves the material contract, not the presence of any engine.

The vLLM planning fixture deliberately separates the service offer from the accelerator offer. This preserves the existing Workcell grammar for later remote, multi-GPU and distributed placement: the service does not become a scheduler or model ontology.

## Physical gates

The default CI suite never claims that Ollama, llama.cpp models, or vLLM/GPU hardware are present. Physical tests are opt-in and fail normally when explicitly enabled but unavailable.

### Ollama service

```bash
WORKCELL_OLLAMA_LIVE=1 \
WORKCELL_OLLAMA_BIN=ollama \
WORKCELL_OLLAMA_PORT=21434 \
./scripts/verify.sh
```

This starts `ollama serve`, proves readiness/observation, then releases the service. Model acquisition is intentionally not implicit in this gate.

### llama.cpp direct CLI + server

```bash
WORKCELL_LLAMA_CPP_LIVE=1 \
WORKCELL_LLAMA_CPP_CLI=llama-cli \
WORKCELL_LLAMA_CPP_SERVER=llama-server \
WORKCELL_LLAMA_CPP_MODEL=/absolute/path/to/model.gguf \
WORKCELL_LLAMA_CPP_PORT=28080 \
./scripts/verify.sh
```

The test first executes `llama-cli` through the ordinary host-process execution port, then starts the same caller-supplied model file through the managed `llama-server` service form.

### vLLM GPU-backed service

```bash
WORKCELL_VLLM_LIVE=1 \
WORKCELL_VLLM_BIN=vllm \
WORKCELL_VLLM_MODEL=<provider-native-model-id> \
WORKCELL_VLLM_PORT=28000 \
./scripts/verify.sh
```

This gate proves process/service/readiness lifecycle on a real vLLM-capable environment. It does not fabricate accelerator evidence: the environment running it must actually satisfy the model and GPU requirements.

## Still physical / external

The repository can deterministically prove lifecycle shape, loss/degradation, logical/material identity separation, endpoint reachability mechanics and accelerator planning. It cannot truthfully claim, from generic hosted CI alone:

- that a particular model has been acquired into an Ollama installation;
- that a real GGUF model loads successfully on the selected llama.cpp build/hardware;
- that a chosen vLLM model fits and runs on an actual accelerator topology;
- that a remote/reference Workcell and its Fabric path are reachable;
- multi-host or multi-GPU performance/placement behaviour.

Those remain explicit physical gates. Their future evidence extends the same `ExecutionDemand` + provider/service/resource/fabric contracts.

Registering the service port on the shipped programs does not close any of them,
and nothing above has been run since this document was written. What changed is
narrower and worth stating exactly: the material contract is now reachable from
the `workcell` binary and `workcell-control-service` instead of only from code
that links the runtime. Whether Ollama, llama.cpp or vLLM actually starts, loads
a model and serves it on a given machine is still decided on that machine, by the
gates above.

## Deliberately absent abstractions

This implementation introduces none of the following:

- `LocalModelProvider`;
- `ModelServer`;
- `ModelRelation`;
- `Harness`;
- `AgentSession`;
- `SessionSpace`;
- Actuation semantics;
- a provider daemon or dynamic plugin ABI.

`epilogos-workcell-sdk` remains a thin public façade over the existing Workcell contracts. No SDK expansion was necessary for this provider implementation.
