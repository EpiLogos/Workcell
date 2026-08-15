# Workcell Wayfinder

GitHub issue #1 is the executable source of the Workcell development graph.

The governing rule is: **do not close a narrow tracer and silently treat it as closure of a wider Workcell responsibility.** Each ticket must prove its public seam, failure modes, provider replacement and architectural non-collapse relevant to that span.

## Foundation line

The first Rust foundation programme established:

1. #2 — F.01 Rust foundation / complete external contract
2. #3 — F.02 ExecutionDemand
3. #4 — F.03 OperationalOffer / planning
4. #5 — F.04 provider port algebra
5. #6 — F.05 WorkspaceProvider
6. #7 — F.09 BindingGraph / MaterialisedExecutionWorld
7. #9 — F.08 ProjectRuntimeProvider / ServiceProvider
8. #17 — expose / collect material operations
9. #10 — F.06 Docker providers
10. #11 — F.11 reconciliation / lifecycle
11. #12 — F.12 deployment profiles / reference Ubuntu Workcell
12. #13 — F.07 optional Arrakis
13. #14 — cross-repository conformance
14. #15 — F.10 Candidate materialisation integration
15. #16 — remote / multi-Workcell placement seam

Deterministic implementation through that line is represented by draft PR #18. Physical Docker and Arrakis acceptance remain independently open under #10 and #13 until actual provider evidence exists.

## Next product frontier — inhabitable Workcells

The next tranche makes the established Workcell semantics directly usable across the current local/server/agent-hosting field without introducing a second architecture:

16. #19 — native CLI and agent-operable Workcell surface
17. #20 — zero-setup collapsed-local Workcell
18. #21 — optional Workcell Control Service and remote control protocol
19. #22 — service bindings and persistent agent-hosting conformance
20. #23 — client SDK, provider SDK and conformance kit
21. #24 — reference Ubuntu remote Workcell and local/server parity
22. #25 — gateway-management interoperability with Hermes and OpenClaw

The governing relations are:

```text
ordinary machine
  native CLI -> Workcell core -> host/filesystem providers

remote/server machine
  client/SDK -> Workcell Control Service -> Workcell core -> providers

persistent agent host / gateway-shaped workload
  higher-layer Agent/Harness/Surface semantics
              ↓ provider-neutral material demand
  Workcell processes + services + bindings + storage + network + lifecycle
```

The Workcell Control Service is optional locally and normal for a remotely controlled server. It is not an agent gateway.

A gateway-shaped workload is a conformance pattern over ordinary Workcell requirements. `Gateway`, `Agent`, `Harness`, `AgentSession`, messaging application and conversation are not Workcell semantic primitives.

Hermes/OpenClaw integrations must be source-pinned against their actual current management surfaces and remain removable adapters. They do not define a universal gateway protocol.

## Execution order

#19 and #20 may start from the existing foundation independently of physical Docker/Arrakis evidence.

#21 follows the native application/CLI seam. #22 composes the zero-setup/local and Control-Service work into persistent-service/agent-hosting conformance. #23 packages stable client/provider extension seams. #24 proves the same demands on the real reference server. #25 then uses real ecosystem gateways as interoperability evidence while coordinating Agent/Harness/Surface semantics with AIKit rather than importing them.

Source-inspection and physical-host gates block only the integrations that genuinely require them. They do not block ordinary deterministic Workcell development.
