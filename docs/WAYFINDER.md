# Workcell Wayfinder

GitHub issue #1 is the executable source of the Workcell development graph.

The governing rule is: **do not close a narrow tracer and silently treat it as closure of a wider F-node.** Each ticket must prove its public seam, its failure modes, provider replacement, and the architectural non-collapse relevant to that span.

Current frontier starts with:

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
15. #16 — remote / multi-Workcell placement open socket

Source-inspection gates block only the provider integrations that genuinely require them. They do not block ordinary Workcell core development.
