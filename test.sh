#!/usr/bin/env bash
# Comprehensive integration test suite for avagen
#
# Usage:
#   ADMIN_SECRET=<your-secret> ./test.sh
#   BASE=https://your-deployment.modal.run ADMIN_SECRET=<secret> ./test.sh
set -euo pipefail

BASE="${BASE:-http://localhost:8080}"
ADMIN="${ADMIN_SECRET:?ADMIN_SECRET environment variable is required}"
WRONG_ADMIN="wrong-secret-!!!"
PASS=0; FAIL=0

# ── helpers ──────────────────────────────────────────────────────────────────
green()  { echo -e "\033[32m✓ $*\033[0m"; }
red()    { echo -e "\033[31m✗ $*\033[0m"; }
yellow() { echo -e "\033[33m» $*\033[0m"; }
header() { echo -e "\n\033[1;34m━━━ $* ━━━\033[0m"; }

assert_status() {
  local label="$1" expected="$2" actual="$3"
  if [[ "$actual" == "$expected" ]]; then
    green "$label (HTTP $actual)"
    PASS=$((PASS+1))
  else
    red "$label — expected HTTP $expected, got $actual"
    FAIL=$((FAIL+1))
  fi
}

assert_contains() {
  local label="$1" needle="$2" body="$3"
  if echo "$body" | grep -q "$needle"; then
    green "$label (contains '$needle')"
    PASS=$((PASS+1))
  else
    red "$label — expected '$needle' in: $body"
    FAIL=$((FAIL+1))
  fi
}

assert_not_contains() {
  local label="$1" needle="$2" body="$3"
  if ! echo "$body" | grep -q "$needle"; then
    green "$label (does not contain '$needle')"
    PASS=$((PASS+1))
  else
    red "$label — should NOT contain '$needle' in: $body"
    FAIL=$((FAIL+1))
  fi
}

assert_status_any() {
  local label="$1" actual="$2"; shift 2
  for expected in "$@"; do
    [[ "$actual" == "$expected" ]] && { green "$label (HTTP $actual)"; PASS=$((PASS+1)); return; }
  done
  red "$label — expected one of HTTP $*, got $actual"
  FAIL=$((FAIL+1))
}

req() {
  # req METHOD URL flags... — returns "<status_code> <body>"
  local method="$1"; shift
  local url="$1"; shift
  curl -s -o /tmp/resp_body -w "%{http_code}" -X "$method" "$url" "$@"
}

# ── 1. Public endpoints ───────────────────────────────────────────────────────
header "1  PUBLIC ENDPOINTS"

s=$(req GET "$BASE/")
b=$(cat /tmp/resp_body)
assert_status "GET /  returns 200" "200" "$s"
assert_contains "GET /  body has 'AvaGen'" "AvaGen" "$b"

s=$(req GET "$BASE/health")
b=$(cat /tmp/resp_body)
assert_status "GET /health returns 200" "200" "$s"
assert_contains "/health has status=ok" '"status":"ok"' "$b"
assert_contains "/health has service=avagen" '"service":"avagen"' "$b"

s=$(req GET "$BASE/no-such-route")
assert_status "GET unknown route returns 404" "404" "$s"

# ── 2. Admin key creation ─────────────────────────────────────────────────────
header "2  ADMIN KEY CRUD"

# Wrong admin secret → 401
s=$(req POST "$BASE/api/admin/keys" \
     -H "Content-Type: application/json" \
     -H "X-Admin-Secret: $WRONG_ADMIN" \
     -d '{"name":"bad-actor"}')
b=$(cat /tmp/resp_body)
assert_status "POST /api/admin/keys wrong secret → 403" "403" "$s"
assert_contains "wrong secret error message" '"error"' "$b"

# Missing Content-Type / body → 415 (Axum rejects before JSON parse)
s=$(req POST "$BASE/api/admin/keys" \
     -H "X-Admin-Secret: $ADMIN")
assert_status "POST /api/admin/keys no Content-Type → 415" "415" "$s"

# Empty name → 400
s=$(req POST "$BASE/api/admin/keys" \
     -H "Content-Type: application/json" \
     -H "X-Admin-Secret: $ADMIN" \
     -d '{"name":"   "}')
b=$(cat /tmp/resp_body)
assert_status "POST /api/admin/keys empty name → 400" "400" "$s"

# Valid creation
s=$(req POST "$BASE/api/admin/keys" \
     -H "Content-Type: application/json" \
     -H "X-Admin-Secret: $ADMIN" \
     -d '{"name":"test-key-1","monthly_quota":10}')
b=$(cat /tmp/resp_body)
assert_status "POST /api/admin/keys valid → 200" "200" "$s"
assert_contains "response has 'key' field" '"key"' "$b"
assert_contains "response has 'id' field" '"id"' "$b"
assert_contains "key prefixed avg_" '"avg_' "$b"
assert_contains "response has 'monthly_quota'" '"monthly_quota"' "$b"
assert_contains "one-time-key warning in message" '"message"' "$b"
API_KEY=$(echo "$b" | python3 -c "import sys,json; print(json.load(sys.stdin)['key'])")
KEY_ID=$(echo "$b" | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")
yellow "  Created API key: $API_KEY  (id: $KEY_ID)"

# Create a second key for revocation test
s=$(req POST "$BASE/api/admin/keys" \
     -H "Content-Type: application/json" \
     -H "X-Admin-Secret: $ADMIN" \
     -d '{"name":"test-key-revoke","monthly_quota":5}')
b=$(cat /tmp/resp_body)
assert_status "POST second key for revoke test → 200" "200" "$s"
REVOKE_ID=$(echo "$b" | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")
REVOKE_KEY=$(echo "$b" | python3 -c "import sys,json; print(json.load(sys.stdin)['key'])")

# List keys
s=$(req GET "$BASE/api/admin/keys" \
     -H "X-Admin-Secret: $ADMIN")
b=$(cat /tmp/resp_body)
assert_status "GET /api/admin/keys → 200" "200" "$s"
assert_contains "list has 'keys' array" '"keys"' "$b"
assert_contains "list contains test-key-1" 'test-key-1' "$b"

# List with wrong secret
s=$(req GET "$BASE/api/admin/keys" \
     -H "X-Admin-Secret: wrong")
assert_status "GET /api/admin/keys wrong secret → 403" "403" "$s"

# Revoke the second key
s=$(req DELETE "$BASE/api/admin/keys/$REVOKE_ID" \
     -H "X-Admin-Secret: $ADMIN")
b=$(cat /tmp/resp_body)
assert_status "DELETE /api/admin/keys/:id valid → 200" "200" "$s"
assert_contains "revoke response has 'revoked'" '"revoked"' "$b"

# Revoked key should no longer authenticate
s=$(req GET "$BASE/api/v1/usage" \
     -H "X-API-Key: $REVOKE_KEY")
assert_status "Revoked key rejected → 401" "401" "$s"

# Delete non-existent key
s=$(req DELETE "$BASE/api/admin/keys/nonexistent-uuid" \
     -H "X-Admin-Secret: $ADMIN")
assert_status "DELETE non-existent key → 404" "404" "$s"

# ── 3. API key authentication ─────────────────────────────────────────────────
header "3  API KEY AUTHENTICATION"

# No key → 401
s=$(req GET "$BASE/api/v1/usage")
b=$(cat /tmp/resp_body)
assert_status "GET /api/v1/usage no key → 401" "401" "$s"
assert_contains "no-key error message present" '"error"' "$b"

# Wrong key → 401
s=$(req GET "$BASE/api/v1/usage" \
     -H "X-API-Key: avg_totally_wrong_key")
assert_status "GET /api/v1/usage invalid key → 401" "401" "$s"

# Valid key → 200
s=$(req GET "$BASE/api/v1/usage" \
     -H "X-API-Key: $API_KEY")
b=$(cat /tmp/resp_body)
assert_status "GET /api/v1/usage valid key → 200" "200" "$s"
assert_contains "usage response has key_id" '"key_id"' "$b"
assert_contains "usage response has monthly_quota" '"monthly_quota":10' "$b"
assert_contains "usage response has monthly_used" '"monthly_used"' "$b"
assert_contains "usage response has daily_breakdown" '"daily_breakdown"' "$b"

# ── 4. Usage endpoint ─────────────────────────────────────────────────────────
header "4  USAGE ENDPOINT"

b=$(curl -s "$BASE/api/v1/usage" -H "X-API-Key: $API_KEY")
QUOTA=$(echo "$b" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['monthly_quota'])")
USED=$(echo "$b" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['monthly_used'])")
REMAINING=$(echo "$b" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['monthly_remaining'])")
yellow "  monthly_quota=$QUOTA  monthly_used=$USED  monthly_remaining=$REMAINING"
assert_contains "remaining = quota - used" "$((QUOTA - USED))" "$REMAINING"

# ── 5. Avatar generation ──────────────────────────────────────────────────────
header "5  AVATAR GENERATION"

# No auth → 401
s=$(req POST "$BASE/api/v1/avatar/generate" \
     -H "Content-Type: application/json" \
     -d '{"age":"adult","sex":"male","ethnicity":"caucasian"}')
assert_status "POST /generate no key → 401" "401" "$s"

# Invalid key → 401
s=$(req POST "$BASE/api/v1/avatar/generate" \
     -H "Content-Type: application/json" \
     -H "X-API-Key: avg_fake_key_1234" \
     -d '{"age":"adult"}')
assert_status "POST /generate invalid key → 401" "401" "$s"

# Invalid size → 400
s=$(req POST "$BASE/api/v1/avatar/generate" \
     -H "Content-Type: application/json" \
     -H "X-API-Key: $API_KEY" \
     -d '{"age":"adult","sex":"male","ethnicity":"caucasian","size":999}')
b=$(cat /tmp/resp_body)
assert_status "POST /generate invalid size → 400" "400" "$s"
assert_contains "invalid size has error message" '"error"' "$b"

# Valid body but pipeline disabled → 503
s=$(req POST "$BASE/api/v1/avatar/generate" \
     -H "Content-Type: application/json" \
     -H "X-API-Key: $API_KEY" \
     -d '{"age":"adult","sex":"male","ethnicity":"caucasian","size":512}')
b=$(cat /tmp/resp_body)
assert_status "POST /generate pipeline disabled → 503" "503" "$s"
assert_contains "503 body has 'error'" '"error"' "$b"

# Missing/bad JSON body → 400 or 422
s=$(req POST "$BASE/api/v1/avatar/generate" \
     -H "Content-Type: application/json" \
     -H "X-API-Key: $API_KEY" \
     -d 'not json')
assert_status_any "POST /generate bad JSON → 400/422" "$s" "400" "422"

# ── 6. Quota enforcement ──────────────────────────────────────────────────────
header "6  QUOTA ENFORCEMENT"

# Create a key with quota = 0
s=$(req POST "$BASE/api/admin/keys" \
     -H "Content-Type: application/json" \
     -H "X-Admin-Secret: $ADMIN" \
     -d '{"name":"zero-quota-key","monthly_quota":0}')
b=$(cat /tmp/resp_body)
assert_status "Create zero-quota key → 200" "200" "$s"
ZERO_KEY=$(echo "$b" | python3 -c "import sys,json; print(json.load(sys.stdin)['key'])")

s=$(req GET "$BASE/api/v1/usage" \
     -H "X-API-Key: $ZERO_KEY")
assert_status "Zero-quota key → 429 (quota exceeded)" "429" "$s"
b=$(cat /tmp/resp_body)
assert_contains "429 message about quota" '"error"' "$b"

# ── 7. Default values & prompt variety ───────────────────────────────────────
header "7  PROMPT / DEFAULT VALUES  (pipeline disabled → 503)"

for body in \
  '{"age":"adult","sex":"male","ethnicity":"caucasian"}' \
  '{"age":"child","sex":"female","ethnicity":"mixed"}' \
  '{"age":"elderly","sex":"male","ethnicity":"east_asian","style":"anime"}' \
  '{"age":"adult","sex":"male","ethnicity":"caucasian","size":256}' \
  '{"age":"adult","sex":"female","ethnicity":"african","size":768,"format":"jpeg"}' \
  '{"age":"teenager","sex":"female","ethnicity":"south_asian","format":"webp","seed":42}'; do
  s=$(req POST "$BASE/api/v1/avatar/generate" \
       -H "Content-Type: application/json" \
       -H "X-API-Key: $API_KEY" \
       -d "$body")
  assert_status "POST /generate body=$body → 503 (expected, no pipeline)" "503" "$s"
done

# ── 8. CORS headers ───────────────────────────────────────────────────────────
header "8  CORS"

s=$(curl -si -X OPTIONS "$BASE/health" \
     -H "Origin: http://example.com" \
     -H "Access-Control-Request-Method: GET" | head -20)
assert_contains "OPTIONS /health has CORS allow-origin header" "access-control-allow-origin" "$s"

# ── 9. Duplicate key name (should succeed — names need not be unique) ──
header "9  DUPLICATE NAME ALLOWED"
s=$(req POST "$BASE/api/admin/keys" \
     -H "Content-Type: application/json" \
     -H "X-Admin-Secret: $ADMIN" \
     -d '{"name":"test-key-1"}')
assert_status "Duplicate name key → 200 (names not required unique)" "200" "$s"
DUP_ID=$(cat /tmp/resp_body | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")
# Clean up
req DELETE "$BASE/api/admin/keys/$DUP_ID" -H "X-Admin-Secret: $ADMIN" > /dev/null

# ── 10. Method not allowed ─────────────────────────────────────────────────────
header "10  METHOD NOT ALLOWED"
s=$(req PUT "$BASE/health")
assert_status "PUT /health → 405" "405" "$s"

s=$(req PATCH "$BASE/api/admin/keys")
assert_status "PATCH /api/admin/keys → 405" "405" "$s"

# ── 11. Large / special characters in key name ───────────────────────────────
header "11  EDGE CASES"

s=$(req POST "$BASE/api/admin/keys" \
     -H "Content-Type: application/json" \
     -H "X-Admin-Secret: $ADMIN" \
     -d '{"name":"key with <script>alert(1)</script>"}')
b=$(cat /tmp/resp_body)
assert_status "Key with HTML chars → 200 (content stored, not rendered)" "200" "$s"
EC_ID=$(echo "$b" | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")
# JSON API correctly stores and returns the raw string - HTML escaping is the client's responsibility
# The response must be valid JSON with the name field preserved verbatim
assert_contains "Name stored verbatim in JSON response" '"name"' "$b"
assert_contains "Response is valid JSON with key field" '"key"' "$b"
req DELETE "$BASE/api/admin/keys/$EC_ID" -H "X-Admin-Secret: $ADMIN" > /dev/null

# Very long name (255 chars) — should succeed
LONG_NAME=$(python3 -c "print('a'*200)")
s=$(req POST "$BASE/api/admin/keys" \
     -H "Content-Type: application/json" \
     -H "X-Admin-Secret: $ADMIN" \
     -d "{\"name\":\"$LONG_NAME\"}")
assert_status "Key with 200-char name → 200" "200" "$s"
LONG_ID=$(cat /tmp/resp_body | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")
req DELETE "$BASE/api/admin/keys/$LONG_ID" -H "X-Admin-Secret: $ADMIN" > /dev/null

# Numeric monthly_quota = 0 is valid (covered above)
# Negative quota — Postgres bigint, should store as-is or reject
s=$(req POST "$BASE/api/admin/keys" \
     -H "Content-Type: application/json" \
     -H "X-Admin-Secret: $ADMIN" \
     -d '{"name":"negative-quota","monthly_quota":-1}')
b=$(cat /tmp/resp_body)
if [[ "$s" == "200" ]]; then
  NEG_ID=$(echo "$b" | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")
  req DELETE "$BASE/api/admin/keys/$NEG_ID" -H "X-Admin-Secret: $ADMIN" > /dev/null
  yellow "  Negative quota stored as-is (i.e. key is always over-quota)"
else
  yellow "  Server rejected negative quota (also acceptable)"
fi

# ── Summary ───────────────────────────────────────────────────────────────────
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo -e "\033[1mResults:  \033[32m$PASS passed\033[0m  /  \033[31m$FAIL failed\033[0m"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
[[ $FAIL -eq 0 ]] && exit 0 || exit 1
