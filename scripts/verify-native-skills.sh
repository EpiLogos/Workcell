#!/usr/bin/env bash
set -euo pipefail

operator="skills/workcell-operation/SKILL.md"
provider="skills/provider-authoring/SKILL.md"

for skill in "$operator" "$provider"; do
  test -f "$skill"
  head -n 1 "$skill" | grep -qx -- '---'
  grep -q '^name:' "$skill"
  grep -q '^description:' "$skill"
  grep -q '^## Contract metadata' "$skill"
  grep -q '^## .*Procedure' "$skill"
done

for operation in status discover plan prepare observe expose collect release reconcile; do
  grep -q "$operation" "$operator"
done
grep -q 'workcell:operator' "$operator"
grep -q 'SecretMaterialisationRequest' "$operator"
grep -q 'SecretMaterialReceipt' "$operator"
grep -q 'Skill available != Capability granted' "$operator"

grep -q 'workcell:provider-developer' "$provider"
grep -q 'verify_provider_port' "$provider"
grep -q 'Workcell-owner review' "$provider"

echo "Workcell native Skills: structural contract OK"
