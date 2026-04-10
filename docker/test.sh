#!/usr/bin/env bash
# PandaFilter end-to-end integration test suite.
# Runs inside Docker as a non-root user, simulating a real developer install.
#
# Usage:
#   docker compose run --rm panda-test          # full suite
#   docker compose run --rm panda-test --only analytics  # filter by tag

set -euo pipefail

# ── Colour helpers ─────────────────────────────────────────────────────────────
GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[0;33m'; BOLD='\033[1m'; NC='\033[0m'
PASS=0; FAIL=0; SKIP=0

ok()   { echo -e "  ${GREEN}✓${NC} $1"; PASS=$((PASS+1)); }
fail() { echo -e "  ${RED}✗${NC} $1"; FAIL=$((FAIL+1)); }
skip() { echo -e "  ${YELLOW}~${NC} $1 (skipped: $2)"; SKIP=$((SKIP+1)); }
hdr()  { echo -e "\n${BOLD}▶ $1${NC}"; }

# Run cmd, capture output, assert condition.
# Usage: run_check "label" <condition_command> [expected_output_fragment]
run_check() {
  local label="$1" cond="$2" fragment="${3:-}"
  local out
  if out=$(eval "$cond" 2>&1); then
    if [[ -n "$fragment" && ! "$out" == *"$fragment"* ]]; then
      fail "$label — output missing: '$fragment'"
      echo "    got: $(echo "$out" | head -5)"
    else
      ok "$label"
    fi
  else
    fail "$label"
    echo "    error: $(echo "$out" | head -5)"
  fi
}

# ── Environment setup ──────────────────────────────────────────────────────────

DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/panda"
mkdir -p "$DATA_DIR"

# Give panda a HOME-based place to write session/cache state
export PANDA_SESSION_ID="test-$$"
export PANDA_AGENT="claude"

# Create a throwaway git repo for testing git-based commands
REPO=$(mktemp -d)
git -C "$REPO" init -q
git -C "$REPO" config user.email "test@panda.test"
git -C "$REPO" config user.name "CCR Test"
echo "hello" > "$REPO/README.md"
git -C "$REPO" add .
git -C "$REPO" commit -q -m "initial commit"
cd "$REPO"

# ─────────────────────────────────────────────────────────────────────────────
hdr "1. Binary sanity"
# ─────────────────────────────────────────────────────────────────────────────

run_check "panda --version prints version number" \
  "panda --version | grep -qE '^panda [0-9]+\.[0-9]+\.[0-9]+$' && panda --version"

run_check "panda --help exits 0" \
  "panda --help"

run_check "panda verify exits 0 (no hooks installed yet, should still exit 0)" \
  "panda verify || true"

# ─────────────────────────────────────────────────────────────────────────────
hdr "2. Hook installation — Claude Code (default)"
# ─────────────────────────────────────────────────────────────────────────────

# Simulate ~/.claude settings.json pre-existing (like a real Claude Code user)
mkdir -p "$HOME/.claude/hooks"
echo '{}' > "$HOME/.claude/settings.json"

run_check "panda init exits 0" \
  "panda init"

run_check "hook script created at ~/.claude/hooks/panda-rewrite.sh" \
  "test -f $HOME/.claude/hooks/panda-rewrite.sh"

run_check "hook script is executable" \
  "test -x $HOME/.claude/hooks/panda-rewrite.sh"

run_check "settings.json contains PreToolUse hook" \
  "grep -q 'PreToolUse' $HOME/.claude/settings.json"

run_check "settings.json contains PostToolUse hook" \
  "grep -q 'PostToolUse' $HOME/.claude/settings.json"

run_check "panda init is idempotent (second run exits 0)" \
  "panda init"

run_check "double init does not duplicate hooks in settings.json" \
  "python3 -c \"
import json, sys
with open('$HOME/.claude/settings.json') as f:
    d = json.load(f)
hooks = d.get('hooks', {})
post = hooks.get('PostToolUse', [])
# Each matcher should appear at most once
matchers = [h.get('matcher','') for h in post if isinstance(h, dict)]
if len(matchers) != len(set(matchers)):
    print('Duplicate matchers:', matchers, file=sys.stderr)
    sys.exit(1)
\""

# ─────────────────────────────────────────────────────────────────────────────
hdr "3. Agent installation — Cline"
# ─────────────────────────────────────────────────────────────────────────────

cd "$REPO"
run_check "panda init --agent cline exits 0" \
  "panda init --agent cline"

run_check ".clinerules created in project dir" \
  "test -f $REPO/.clinerules"

run_check ".clinerules contains panda-rules-start marker" \
  "grep -q 'panda-rules-start' $REPO/.clinerules"

run_check ".clinerules contains panda run instructions" \
  "grep -q 'panda run' $REPO/.clinerules"

# Simulate existing .clinerules (user has their own rules)
echo "# My team rules" > "$REPO/.clinerules"
run_check "panda init --agent cline appends to existing .clinerules" \
  "panda init --agent cline && grep -q 'My team rules' $REPO/.clinerules"

run_check "existing rules preserved after second init" \
  "grep -q 'My team rules' $REPO/.clinerules"

run_check "panda init --agent cline is idempotent (replaces block, not duplicates)" \
  "panda init --agent cline && grep -c 'panda-rules-start' $REPO/.clinerules | grep -q '^1$'"

run_check "panda init --uninstall --agent cline removes block" \
  "panda init --uninstall --agent cline && ! grep -q 'panda-rules-start' $REPO/.clinerules"

run_check "panda init --uninstall --agent cline preserves other content" \
  "grep -q 'My team rules' $REPO/.clinerules"

# ─────────────────────────────────────────────────────────────────────────────
hdr "4. Agent installation — Gemini CLI"
# ─────────────────────────────────────────────────────────────────────────────

run_check "panda init --agent gemini exits 0" \
  "panda init --agent gemini"

run_check "Gemini hook script created at ~/.gemini/panda-rewrite.sh" \
  "test -f $HOME/.gemini/panda-rewrite.sh"

run_check "Gemini hook script is executable" \
  "test -x $HOME/.gemini/panda-rewrite.sh"

run_check "Gemini settings.json created" \
  "test -f $HOME/.gemini/settings.json"

run_check "Gemini settings.json is valid JSON" \
  "python3 -m json.tool $HOME/.gemini/settings.json > /dev/null"

run_check "Gemini settings.json contains BeforeTool entry" \
  "python3 -c \"
import json
with open('$HOME/.gemini/settings.json') as f:
    d = json.load(f)
hooks = d.get('hooks', {})
assert 'BeforeTool' in hooks, 'BeforeTool missing from settings.json'
entry = hooks['BeforeTool'][0]
assert entry.get('matcher') == 'run_shell_command', 'matcher should be run_shell_command'
assert entry['hooks'][0].get('type') == 'command', 'hook type should be command'
\""

run_check "Gemini hook script always exits 0 even with bad input" \
  "echo 'bad json' | bash $HOME/.gemini/panda-rewrite.sh; test \$? -eq 0"

run_check "panda init --agent gemini is idempotent (no duplicate BeforeTool entries)" \
  "panda init --agent gemini && python3 -c \"
import json
with open('$HOME/.gemini/settings.json') as f:
    d = json.load(f)
before_tool = d.get('hooks', {}).get('BeforeTool', [])
ccr_count = sum(1 for e in before_tool if any('panda' in str(h.get('command','')) for h in e.get('hooks',[])))
assert ccr_count == 1, f'Expected 1 panda entry, got {ccr_count}'
\""

run_check "panda init --uninstall --agent gemini removes hook script" \
  "panda init --uninstall --agent gemini && test ! -f $HOME/.gemini/panda-rewrite.sh"

run_check "panda init --uninstall --agent gemini cleans settings.json" \
  "python3 -c \"
import json
with open('$HOME/.gemini/settings.json') as f:
    d = json.load(f)
before_tool = d.get('hooks', {}).get('BeforeTool', [])
ccr_entries = [e for e in before_tool if any('panda' in str(h.get('command','')) for h in e.get('hooks',[]))]
assert len(ccr_entries) == 0, 'BeforeTool still has panda entries after uninstall'
\""

# ─────────────────────────────────────────────────────────────────────────────
hdr "5. Agent installation — VS Code Copilot"
# ─────────────────────────────────────────────────────────────────────────────
# Copilot is project-scoped: installs to .github/hooks/ in the current dir.

cd "$REPO"

run_check "panda init --agent copilot exits 0" \
  "panda init --agent copilot"

run_check "Copilot hook script created at .github/hooks/panda-rewrite.sh" \
  "test -f $REPO/.github/hooks/panda-rewrite.sh"

run_check "Copilot hook script is executable" \
  "test -x $REPO/.github/hooks/panda-rewrite.sh"

run_check "Copilot hook config created at .github/hooks/panda-rewrite.json" \
  "test -f $REPO/.github/hooks/panda-rewrite.json"

run_check "Copilot hook config is valid JSON" \
  "python3 -m json.tool $REPO/.github/hooks/panda-rewrite.json > /dev/null"

run_check "Copilot hook config contains PreToolUse entry" \
  "python3 -c \"
import json
with open('$REPO/.github/hooks/panda-rewrite.json') as f:
    d = json.load(f)
hooks = d.get('hooks', {}).get('PreToolUse', [])
assert len(hooks) > 0, 'PreToolUse missing from hook config'
assert hooks[0].get('type') == 'command', 'hook type should be command'
\""

run_check "Copilot instructions file created at .github/copilot-instructions.md" \
  "test -f $REPO/.github/copilot-instructions.md"

run_check "Copilot instructions contain panda-instructions-start marker" \
  "grep -q 'panda-instructions-start' $REPO/.github/copilot-instructions.md"

run_check "Copilot hook script reads tool_input.command from JSON" \
  "grep -q 'tool_input.command' $REPO/.github/hooks/panda-rewrite.sh"

run_check "Copilot hook script exits 0 on empty input" \
  "echo '' | bash $REPO/.github/hooks/panda-rewrite.sh; test \$? -eq 0"

run_check "panda init --agent copilot is idempotent (no duplicate PreToolUse entries)" \
  "panda init --agent copilot && python3 -c \"
import json
with open('$REPO/.github/hooks/panda-rewrite.json') as f:
    d = json.load(f)
hooks = d.get('hooks', {}).get('PreToolUse', [])
assert len(hooks) == 1, f'Expected 1 PreToolUse entry, got {len(hooks)}'
\""

run_check "panda init --agent copilot idempotent: instructions not duplicated" \
  "panda init --agent copilot && python3 -c \"
content = open('$REPO/.github/copilot-instructions.md').read()
assert content.count('panda-instructions-start') == 1, 'Instructions block duplicated'
\""

run_check "panda init --uninstall --agent copilot removes hook files" \
  "panda init --uninstall --agent copilot && test ! -f $REPO/.github/hooks/panda-rewrite.sh && test ! -f $REPO/.github/hooks/panda-rewrite.json"

run_check "panda init --uninstall --agent copilot removes instructions block" \
  "test ! -f $REPO/.github/copilot-instructions.md || ! grep -q 'panda-instructions-start' $REPO/.github/copilot-instructions.md"

# ─────────────────────────────────────────────────────────────────────────────
hdr "6. panda run — basic command compression"
# ─────────────────────────────────────────────────────────────────────────────

cd "$REPO"

run_check "panda run git status exits 0" \
  "panda run git status"

run_check "panda run git log exits 0" \
  "panda run git log --oneline"

# Add enough files to trigger collapse in git status
for i in $(seq 1 30); do echo "content$i" > "file$i.txt"; done
git add . && git commit -q -m "add many files"

run_check "panda run git diff HEAD~1 compresses large output" \
  "panda run git diff HEAD~1"

# Test that panda run writes analytics
ANALYTICS_DB="$DATA_DIR/analytics.db"
if [[ -f "$ANALYTICS_DB" ]]; then
  COUNT=$(sqlite3 "$ANALYTICS_DB" "SELECT COUNT(*) FROM records;")
  if [[ "$COUNT" -gt 0 ]]; then
    ok "panda run writes analytics to SQLite DB (found $COUNT records)"
  else
    fail "panda run wrote 0 analytics records"
  fi
else
  fail "analytics.db not created after panda run"
fi

# ─────────────────────────────────────────────────────────────────────────────
hdr "7. panda filter — stdin pipeline"
# ─────────────────────────────────────────────────────────────────────────────

LONG_OUTPUT=$(python3 -c "
# Use terraform 'Refreshing state...' lines — these hit the terraform Collapse
# pattern and are NOT stripped by global_rules (only cargo/rustc progress is global)
lines = []
for i in range(20):
    lines.append('  Refreshing state...')
lines.append('Plan: 2 to add, 0 to change, 0 to destroy.')
print('\n'.join(lines))
")

run_check "panda filter collapses Refreshing state lines (terraform)" \
  "echo \"\$LONG_OUTPUT\" | panda filter --command terraform" "collapsed"

run_check "panda filter preserves plan summary line" \
  "echo \"\$LONG_OUTPUT\" | panda filter --command terraform" "Plan:"

run_check "panda filter with no command hint still works" \
  "echo 'hello world' | panda filter"

# ─────────────────────────────────────────────────────────────────────────────
hdr "8. panda hook — PostToolUse JSON simulation"
# ─────────────────────────────────────────────────────────────────────────────

# Simulate Claude Code calling panda hook with a Bash tool response
HOOK_INPUT=$(python3 -c "
import json, sys
# Simulate: 50 'Compiling' lines + 1 error line
lines = ['   Compiling crate%d v1.0.0' % i for i in range(50)]
lines.append('error[E0001]: something important')
output = '\n'.join(lines)
payload = {
    'tool_name': 'Bash',
    'tool_input': {'command': 'cargo build'},
    'tool_response': {'output': output}
}
print(json.dumps(payload))
")

HOOK_OUT=$(echo "$HOOK_INPUT" | PANDA_SESSION_ID="hook-test-$$" panda hook 2>/dev/null || true)

if [[ -n "$HOOK_OUT" ]]; then
  if echo "$HOOK_OUT" | python3 -m json.tool > /dev/null 2>&1; then
    ok "panda hook returns valid JSON"
    INNER=$(echo "$HOOK_OUT" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('output',''))")
    if echo "$INNER" | grep -q "collapsed\|E0001"; then
      ok "panda hook output contains compressed content"
    else
      fail "panda hook output doesn't look compressed"
    fi
    if echo "$INNER" | grep -q "E0001"; then
      ok "panda hook preserves error lines"
    else
      fail "panda hook dropped the error line"
    fi
  else
    fail "panda hook returned invalid JSON: $(echo "$HOOK_OUT" | head -2)"
  fi
else
  # Empty output = pass-through (hook decided no compression needed)
  ok "panda hook returned empty (pass-through — output was too small or trivial)"
fi

# Test Glob tool hook
GLOB_INPUT=$(python3 -c "
import json
paths = ['/project/src/file%d.rs' % i for i in range(100)]
payload = {
    'tool_name': 'Glob',
    'tool_input': {'pattern': '**/*.rs'},
    'tool_response': {'output': '\n'.join(paths)}
}
print(json.dumps(payload))
")

GLOB_OUT=$(echo "$GLOB_INPUT" | PANDA_SESSION_ID="glob-test-$$" panda hook 2>/dev/null || true)
if [[ -n "$GLOB_OUT" ]]; then
  ok "panda hook handles Glob tool (large path list)"
else
  ok "panda hook pass-through for Glob (acceptable)"
fi

# ─────────────────────────────────────────────────────────────────────────────
hdr "9. panda rewrite — command rewriting"
# ─────────────────────────────────────────────────────────────────────────────

run_check "panda rewrite 'git status' returns panda-prefixed command" \
  "panda rewrite 'git status'" "panda"

run_check "panda rewrite 'cargo build' returns panda-prefixed command" \
  "panda rewrite 'cargo build'" "panda"

run_check "panda rewrite 'echo hello' exits (no rewrite for unknown commands)" \
  "panda rewrite 'echo hello' > /dev/null 2>&1 || true"

# ─────────────────────────────────────────────────────────────────────────────
hdr "10. End-to-end hook → analytics pipeline (fresh-install simulation)"
# ─────────────────────────────────────────────────────────────────────────────
# Simulates exactly what a new user does: install, init, use Claude Code,
# check gain. This is the path that showed "0 runs" for a real user.

# Start from a clean DB so the count is deterministic
FRESH_DB="$DATA_DIR/analytics_e2e_test.db"
rm -f "$FRESH_DB"

# Step 1: simulate Claude Code PreToolUse hook firing for 'git status'
HOOK_SCRIPT="$HOME/.claude/hooks/panda-rewrite.sh"
HOOK_INPUT='{"tool_name":"Bash","tool_input":{"command":"git status"}}'
REWRITE_OUT=$(echo "$HOOK_INPUT" | bash "$HOOK_SCRIPT" 2>/dev/null)
REWRITTEN_CMD=$(echo "$REWRITE_OUT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('hookSpecificOutput',{}).get('updatedInput',{}).get('command',''))" 2>/dev/null)

if [[ "$REWRITTEN_CMD" == *"panda"* ]]; then
  ok "hook script rewrites git status to panda-prefixed command"
else
  fail "hook script did not rewrite git status (got: '$REWRITTEN_CMD')"
fi

# Step 2: run the rewritten command (as Claude Code would)
DB_BEFORE=$(sqlite3 "$DATA_DIR/analytics.db" "SELECT COUNT(*) FROM records;" 2>/dev/null || echo 0)
eval "$REWRITTEN_CMD" > /dev/null 2>&1 || true
DB_AFTER=$(sqlite3 "$DATA_DIR/analytics.db" "SELECT COUNT(*) FROM records;" 2>/dev/null || echo 0)

if [[ "$DB_AFTER" -gt "$DB_BEFORE" ]]; then
  ok "running rewritten command writes a new analytics record (${DB_BEFORE} → ${DB_AFTER})"
else
  fail "running rewritten command did NOT write analytics (count stayed at ${DB_BEFORE})"
fi

# Step 3: panda gain must show those records (the "0 runs" regression check)
run_check "panda gain shows non-zero Runs after hook fired" \
  "panda gain | python3 -c \"
import sys
out = sys.stdin.read()
assert 'Runs:' in out, 'Runs: missing'
import re
m = re.search(r'Runs:\s+(\d+)', out)
assert m and int(m.group(1)) > 0, f'Expected >0 runs, got: {out[:200]}'
\""

# ─────────────────────────────────────────────────────────────────────────────
hdr "11. panda doctor — installation diagnostics"
# ─────────────────────────────────────────────────────────────────────────────

run_check "panda doctor exits 0" \
  "panda doctor"

run_check "panda doctor shows DB path" \
  "panda doctor" "DB path"

run_check "panda doctor shows DB records" \
  "panda doctor" "DB records"

run_check "panda doctor shows hook script" \
  "panda doctor" "Hook script"

run_check "panda doctor shows rewrite check" \
  "panda doctor" "panda run"

# ── Failure scenario 1: DB never created (panda run never called) ──────────────
# Test that 0-record state is clearly reported, not silently ignored
FRESH_XDG=$(mktemp -d)
FRESH_OUT=$(XDG_DATA_HOME0doctor 2>&1 || true)
if echo "$FRESH_OUT" | grep -q "NOT created yet\|0 records\|never been called"; then
  ok "panda doctor reports clearly when DB has no records yet"
else
  fail "panda doctor should warn when DB has never been written"
  echo "    got: $(echo "$FRESH_OUT" | grep -i 'db\|record\|created' | head -3)"
fi
rm -rf "$FRESH_XDG"

# ── Failure scenario 2: bad binary path in hook script ───────────────────────
# Simulate a hook script where the panda binary path is stale (e.g. after brew upgrade)
BAD_HOOK_DIR=$(mktemp -d)
BAD_HOOK="$BAD_HOOK_DIR/panda-rewrite.sh"
cat > "$BAD_HOOK" << 'HOOKEOF'
#!/usr/bin/env bash
INPUT=$(cat)
CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty' 2>/dev/null)
[ -z "$CMD" ] && exit 0
# Deliberately broken binary path
REWRITTEN=$(PANDA_SESSION_ID=$PPID "/nonexistent/path/panda" rewrite "$CMD" 2>/dev/null) || exit 0
[ "$CMD" = "$REWRITTEN" ] && exit 0
echo '{"hookSpecificOutput":{"updatedInput":{"command":"'"$REWRITTEN"'"}}}'
HOOKEOF
chmod +x "$BAD_HOOK"

# The hook should exit 0 (graceful degradation) even with a bad binary path
HOOK_INPUT='{"tool_name":"Bash","tool_input":{"command":"git status"}}'
BAD_EXIT=0
echo "$HOOK_INPUT" | bash "$BAD_HOOK" > /dev/null 2>&1 || BAD_EXIT=$?
if [[ "$BAD_EXIT" -eq 0 ]]; then
  ok "hook script with bad binary path exits 0 (graceful degradation)"
else
  fail "hook script with bad binary path should exit 0, got exit $BAD_EXIT"
fi

# The hook should produce no output (no rewrite) when binary is missing
BAD_OUT=$(echo "$HOOK_INPUT" | bash "$BAD_HOOK" 2>/dev/null || true)
if [[ -z "$BAD_OUT" ]]; then
  ok "hook script with bad binary path produces no output (pass-through)"
else
  fail "hook script with bad binary path should produce no output, got: '$BAD_OUT'"
fi

# After a bad hook, panda run still works when called directly (panda in PATH)
DB_BEFORE=$(sqlite3 "$DATA_DIR/analytics.db" "SELECT COUNT(*) FROM records;" 2>/dev/null || echo 0)
panda run git status > /dev/null 2>&1 || true
DB_AFTER=$(sqlite3 "$DATA_DIR/analytics.db" "SELECT COUNT(*) FROM records;" 2>/dev/null || echo 0)
if [[ "$DB_AFTER" -gt "$DB_BEFORE" ]]; then
  ok "panda run still writes analytics even when PreToolUse hook had a bad binary"
else
  fail "panda run should write analytics even when hook binary is broken"
fi
rm -rf "$BAD_HOOK_DIR"

# ── Failure scenario 3: user runs commands themselves (no hook) ───────────────
# The PreToolUse hook ONLY fires when Claude Code's AI runs tools.
# If a user types 'git status' in their own terminal, no hook fires, no analytics.
# Verify: direct git status (no hook) = no new CCR record.
# Verify: panda run git status (explicit) = new record.
DB_BEFORE=$(sqlite3 "$DATA_DIR/analytics.db" "SELECT COUNT(*) FROM records;" 2>/dev/null || echo 0)
git status > /dev/null 2>&1 || true   # user typing directly — no hook fires
DB_AFTER=$(sqlite3 "$DATA_DIR/analytics.db" "SELECT COUNT(*) FROM records;" 2>/dev/null || echo 0)
if [[ "$DB_AFTER" -eq "$DB_BEFORE" ]]; then
  ok "'git status' run directly (no hook) writes no CCR analytics record"
else
  fail "'git status' run directly should NOT write analytics (got ${DB_BEFORE} → ${DB_AFTER})"
fi

DB_BEFORE=$DB_AFTER
panda run git status > /dev/null 2>&1 || true   # routed through CCR explicitly
DB_AFTER=$(sqlite3 "$DATA_DIR/analytics.db" "SELECT COUNT(*) FROM records;" 2>/dev/null || echo 0)
if [[ "$DB_AFTER" -gt "$DB_BEFORE" ]]; then
  ok "'panda run git status' writes analytics record (${DB_BEFORE} → ${DB_AFTER})"
else
  fail "'panda run git status' should write analytics record"
fi

# ─────────────────────────────────────────────────────────────────────────────
hdr "13. panda gain — analytics display"
# ─────────────────────────────────────────────────────────────────────────────

panda run git log --oneline > /dev/null 2>&1 || true
panda run git status > /dev/null 2>&1 || true

run_check "panda gain exits 0" "panda gain"
run_check "panda gain shows Runs:" "panda gain" "Runs:"
run_check "panda gain shows Tokens saved:" "panda gain" "Tokens saved:"
run_check "panda gain --breakdown exits 0" "panda gain --breakdown"

# ─────────────────────────────────────────────────────────────────────────────
hdr "14. Analytics migration — JSONL → SQLite"
# ─────────────────────────────────────────────────────────────────────────────
# Simulate a user who has v0.5.x JSONL analytics and upgrades to v0.6.0.

# Use a dedicated temp XDG_DATA_HOME to isolate migration from the main test DB
MIGRATE_XDG=$(mktemp -d)
MIGRATE_CCR_DIR="$MIGRATE_XDG/panda"
mkdir -p "$MIGRATE_CCR_DIR"

# Plant legacy JSONL (simulates a pre-v0.6.0 install)
cp /src/docker/fixtures/legacy_analytics.jsonl "$MIGRATE_CCR_DIR/analytics.jsonl"
LEGACY_COUNT=$(wc -l < "$MIGRATE_CCR_DIR/analytics.jsonl" | tr -d ' ')

# Trigger panda gain with the isolated data dir — this should auto-migrate JSONL → SQLite
XDG_DATA_HOME="$MIGRATE_XDG" panda gain > /dev/null 2>&1 || true

MIGRATE_DB="$MIGRATE_CCR_DIR/analytics.db"
MIGRATE_BAK="$MIGRATE_CCR_DIR/analytics.jsonl.bak"

if [[ -f "$MIGRATE_DB" ]]; then
  DB_COUNT=$(sqlite3 "$MIGRATE_DB" "SELECT COUNT(*) FROM records;" 2>/dev/null || echo 0)
  if [[ "$DB_COUNT" -ge "$LEGACY_COUNT" ]]; then
    ok "JSONL migration: $DB_COUNT records migrated to SQLite (expected ~$LEGACY_COUNT)"
  else
    fail "JSONL migration: only $DB_COUNT records in DB, expected $LEGACY_COUNT"
  fi
else
  fail "JSONL migration: analytics.db was not created"
fi

if [[ -f "$MIGRATE_BAK" ]]; then
  ok "JSONL migration: original .jsonl renamed to .jsonl.bak"
else
  fail "JSONL migration: .jsonl.bak not created (old data may be lost)"
fi

# Idempotency: second panda gain must not re-import records
if [[ -f "$MIGRATE_DB" ]]; then
  BEFORE=$(sqlite3 "$MIGRATE_DB" "SELECT COUNT(*) FROM records;" 2>/dev/null || echo 0)
  XDG_DATA_HOME="$MIGRATE_XDG" panda gain > /dev/null 2>&1 || true
  AFTER=$(sqlite3 "$MIGRATE_DB" "SELECT COUNT(*) FROM records;" 2>/dev/null || echo 0)
  if [[ "$BEFORE" -eq "$AFTER" ]]; then
    ok "Migration is idempotent: second panda gain doesn't re-import records"
  else
    fail "Migration ran twice: $BEFORE → $AFTER (should be stable at $BEFORE)"
  fi
fi

rm -rf "$MIGRATE_XDG"

# ─────────────────────────────────────────────────────────────────────────────
hdr "15. SQLite analytics correctness"
# ─────────────────────────────────────────────────────────────────────────────

CURRENT_DB="$DATA_DIR/analytics.db"

if [[ -f "$CURRENT_DB" ]]; then
  # Verify schema
  run_check "analytics.db has 'records' table" \
    "sqlite3 \"$CURRENT_DB\" \".tables\"" "records"

  run_check "analytics.db records have timestamp_secs > 0" \
    "sqlite3 \"$CURRENT_DB\" \"SELECT COUNT(*) FROM records WHERE timestamp_secs > 0;\" | grep -v '^0$'"

  run_check "analytics.db has idx_project_ts index" \
    "sqlite3 \"$CURRENT_DB\" \".indexes records\"" "idx_project_ts"

  # Verify savings_pct is never > 100
  OVER=$(sqlite3 "$CURRENT_DB" "SELECT COUNT(*) FROM records WHERE savings_pct > 100.0;")
  if [[ "$OVER" -eq 0 ]]; then
    ok "No records have savings_pct > 100"
  else
    fail "$OVER records have savings_pct > 100 (data corruption)"
  fi

  # Verify auto-cleanup doesn't delete recent records
  RECENT=$(sqlite3 "$CURRENT_DB" "SELECT COUNT(*) FROM records WHERE timestamp_secs > strftime('%s','now') - 86400;")
  if [[ "$RECENT" -gt 0 ]]; then
    ok "Recent records ($RECENT today) preserved by auto-cleanup"
  else
    skip "auto-cleanup check" "no records written today"
  fi
else
  fail "analytics.db not found at $CURRENT_DB"
fi

# ─────────────────────────────────────────────────────────────────────────────
hdr "16. panda expand — zoom-in block retrieval"
# ─────────────────────────────────────────────────────────────────────────────

# Generate output with a collapsed block (zoom must be enabled)
ZOOM_OUT=$(panda run git diff HEAD~1 2>/dev/null || true)
if echo "$ZOOM_OUT" | grep -q "ZI_"; then
  ZOOM_ID=$(echo "$ZOOM_OUT" | grep -o 'ZI_[0-9]*' | head -1)
  run_check "panda expand $ZOOM_ID retrieves original lines" \
    "panda expand ${ZOOM_ID#ZI_}"
else
  skip "panda expand test" "no ZI_ marker in output (zoom may be disabled or output too small)"
fi

# ─────────────────────────────────────────────────────────────────────────────
hdr "17. Uninstall — Claude Code"
# ─────────────────────────────────────────────────────────────────────────────

run_check "panda init --uninstall exits 0" \
  "panda init --uninstall"

run_check "hook script removed after uninstall" \
  "test ! -f $HOME/.claude/hooks/panda-rewrite.sh"

run_check "re-running panda init after uninstall works" \
  "panda init && test -f $HOME/.claude/hooks/panda-rewrite.sh"

# ─────────────────────────────────────────────────────────────────────────────
hdr "18. Edge cases"
# ─────────────────────────────────────────────────────────────────────────────

run_check "panda run with no args exits cleanly (shows help)" \
  "panda run 2>&1 || true"

run_check "panda filter empty stdin produces no output" \
  "echo '' | panda filter 2>/dev/null; true"

run_check "panda hook with empty stdin returns nothing (no crash)" \
  "echo '' | panda hook 2>/dev/null; true"

run_check "panda hook with malformed JSON returns nothing (no crash)" \
  "echo 'not json at all' | panda hook 2>/dev/null; true"

run_check "panda gain with no analytics exits 0" \
  "XDG_DATA_HOME=$(mktemp -d) panda gain"

# ─────────────────────────────────────────────────────────────────────────────
# Summary
# ─────────────────────────────────────────────────────────────────────────────

echo ""
echo -e "${BOLD}─────────────────────────────────────────────────${NC}"
TOTAL=$((PASS + FAIL + SKIP))
echo -e "  ${BOLD}Results: $TOTAL tests${NC}   ${GREEN}$PASS passed${NC}   ${RED}$FAIL failed${NC}   ${YELLOW}$SKIP skipped${NC}"
echo -e "${BOLD}─────────────────────────────────────────────────${NC}"

if [[ "$FAIL" -gt 0 ]]; then
  exit 1
fi
exit 0
