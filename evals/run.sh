#!/bin/bash
# Asks `claude -p` each question in evals/questions.xml with only this server attached
# (read-only: no --allow-write) and checks that the answer appears in the reply.
#   eval "$(evals/seed.sh)" && cargo build && evals/run.sh
# Costs model tokens; the answers come from the seeded mailbox, so re-seed between runs.
set -euo pipefail
cd "$(dirname "$0")/.."
: "${JMAP_URL:?run eval \"\$(evals/seed.sh)\" first}" "${JMAP_TOKEN:?}"
BIN=$PWD/target/debug/jmap-mcp
CFG=$(mktemp)
trap 'rm -f "$CFG"' EXIT
jq -n --arg bin "$BIN" --arg url "$JMAP_URL" --arg tok "$JMAP_TOKEN" \
  '{mcpServers:{jmap:{command:$bin,env:{JMAP_URL:$url,JMAP_TOKEN:$tok}}}}' >"$CFG"

mapfile -t Q < <(grep -oP '(?<=<question>).*(?=</question>)' evals/questions.xml)
mapfile -t A < <(grep -oP '(?<=<answer>).*(?=</answer>)' evals/questions.xml)
pass=0
for i in "${!Q[@]}"; do
  # stdout only, last line only: the reply, not tool or schema noise on stderr.
  out=$(claude -p "${Q[$i]} Answer with just the answer." --strict-mcp-config --mcp-config "$CFG" \
    --allowedTools 'mcp__jmap__*' --model "${EVAL_MODEL:-sonnet}" --max-turns 12 2>/dev/null | tail -1)
  if grep -qiF -- "${A[$i]}" <<<"$out"; then r=pass; pass=$((pass + 1)); else r=FAIL; fi
  printf '%-4s q%-2s expected %-18s got: %s\n' "$r" "$((i + 1))" "${A[$i]}" "$(tr '\n' ' ' <<<"$out" | cut -c1-80)"
done
echo "result: $pass/${#Q[@]}"
[ "$pass" -eq "${#Q[@]}" ]
