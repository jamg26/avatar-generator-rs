#!/usr/bin/env bash
# deploy.sh — single-shot full deployment for AvaGen
#
# Pushes to:
#   • GitHub (origin/main)
#   • HuggingFace Spaces (space/main) — triggers Docker rebuild
# Then syncs Space secrets from .env.
#
# Usage:
#   ./deploy.sh                        # deploy current HEAD (must be clean)
#   ./deploy.sh "feat: your message"   # commit everything, then deploy

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$REPO_ROOT"

# ── Colours ───────────────────────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; CYAN='\033[0;36m'; BOLD='\033[1m'; NC='\033[0m'
info()    { echo -e "${CYAN}→ $*${NC}"; }
success() { echo -e "${GREEN}✓ $*${NC}"; }
die()     { echo -e "${RED}ERROR: $*${NC}" >&2; exit 1; }

# ── Load .env ─────────────────────────────────────────────────────────────────
if [[ -f .env ]]; then
    set -a; source .env; set +a
fi

[[ -n "${HF_TOKEN:-}" ]] || die "HF_TOKEN not set (add to .env or export it)"

# ── Optional commit ───────────────────────────────────────────────────────────
MSG="${1:-}"
if [[ -n "$(git status --porcelain)" ]]; then
    [[ -n "$MSG" ]] || die "Working tree has uncommitted changes.\nPass a commit message: ./deploy.sh \"your message\""
    info "Committing changes: $MSG"
    git add -A
    git commit -m "$MSG

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
fi

COMMIT=$(git rev-parse --short HEAD)
echo -e "${BOLD}Deploying ${COMMIT} …${NC}"

# ── Push to GitHub ────────────────────────────────────────────────────────────
info "Pushing to GitHub (origin/main) …"
git push origin main
success "GitHub updated"

# ── Push to HuggingFace Spaces ────────────────────────────────────────────────
info "Pushing to HuggingFace Spaces (space/main) …"
git push space main
success "HF Space build triggered"

# ── Sync secrets ─────────────────────────────────────────────────────────────
info "Syncing Space secrets …"

PYTHON="${PYTHON:-$(command -v python3 2>/dev/null || command -v python)}"
[[ -n "$PYTHON" ]] || die "python3 not found — install it to sync secrets"

$PYTHON - <<'PYEOF'
import os, sys

try:
    from huggingface_hub import HfApi
except ImportError:
    os.system(f"{sys.executable} -m pip install -q huggingface_hub")
    from huggingface_hub import HfApi

SPACE_ID = "jamg/avagen"
hf_token = os.environ["HF_TOKEN"]
api = HfApi(token=hf_token)

# Ensure the Space exists
api.create_repo(repo_id=SPACE_ID, repo_type="space", space_sdk="docker",
                private=False, exist_ok=True)

secrets = {
    "DATABASE_URL":        os.environ.get("DATABASE_URL"),
    "ADMIN_SECRET":        os.environ.get("ADMIN_SECRET"),
    "HF_TOKEN":            hf_token,
    "REPLICATE_API_TOKEN": os.environ.get("REPLICATE_API_TOKEN"),
    "STABLE_HORDE_KEY":    os.environ.get("STABLE_HORDE_KEY"),
    "HF_BUCKET_ID":        os.environ.get("HF_BUCKET_ID"),
}
for key, value in secrets.items():
    if value:
        try:
            api.add_space_secret(repo_id=SPACE_ID, key=key, value=value)
            print(f"  secret set: {key}")
        except Exception as exc:
            print(f"  warning: could not set {key}: {exc}", file=sys.stderr)
PYEOF

# ── Done ──────────────────────────────────────────────────────────────────────
echo ""
echo -e "${BOLD}${GREEN}Deployed!${NC}"
echo -e "  Space:    https://huggingface.co/spaces/jamg/avagen"
echo -e "  API:      https://jamg-avagen.hf.space"
echo -e "  GitHub:   https://github.com/jamg26/avatar-generator-rs"
echo -e ""
echo -e "The Space is rebuilding (Rust compile ~2 min)."
echo -e "When live, smoke-test with:"
echo -e "  BASE=https://jamg-avagen.hf.space ADMIN_SECRET=\$ADMIN_SECRET ./test.sh"
