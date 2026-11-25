#!/bin/bash
# Starts a throwaway postmark2jmap with a fixed mailbox for bob@example.com and prints the
# environment to use it:   eval "$(evals/seed.sh)"
# Needs a built postmark2jmap (P2J_BIN, default ~/projects/postmarkapp2jmapserver/target/release/postmark2jmap).
set -euo pipefail
B=${P2J_BIN:-$HOME/projects/postmarkapp2jmapserver/target/release/postmark2jmap}
D=${JMAP_DEV_DIR:-/tmp/jmap-mcp-dev}
PORT=${JMAP_DEV_PORT:-18080}
URL=http://127.0.0.1:$PORT

[ -f "$D/pid" ] && kill "$(cat "$D/pid")" 2>/dev/null || true
rm -rf "$D"; mkdir -p "$D/data"
export P2J_DATA=$D/data P2J_KEY_FILE=$D/key
"$B" keygen "$D/key" >/dev/null
"$B" domain example.com >/dev/null
"$B" account bob@example.com >/dev/null
"$B" account alice@example.com >/dev/null
TOKEN=$("$B" token bob@example.com)
P2J_LISTEN=127.0.0.1:$PORT P2J_PUBLIC_URL=$URL nohup "$B" serve >"$D/server.log" 2>&1 &
echo $! >"$D/pid"
for _ in $(seq 50); do curl -sf -o /dev/null -H "Authorization: Bearer $TOKEN" "$URL/jmap/session" && break; sleep 0.2; done

jmap() { curl -s -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' "$URL/jmap" -d "$1"; }

# msg <from> <to> <cc> <subject> <message-id> <in-reply-to> <date> <body>
blob() {
  local hdr="From: $1\r\nTo: $2\r\n"
  [ -n "$3" ] && hdr+="Cc: $3\r\n"
  hdr+="Subject: $4\r\nMessage-ID: <$5>\r\n"
  [ -n "$6" ] && hdr+="In-Reply-To: <$6>\r\nReferences: <$6>\r\n"
  hdr+="Date: $7\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n"
  printf "$hdr%s\r\n" "$8" | curl -s -H "Authorization: Bearer $TOKEN" -H 'content-type: message/rfc822' \
    --data-binary @- "$URL/jmap/upload/a1" | jq -r .blobId
}
IMPORTS='{}'
add() { # key blob received keywords-json
  IMPORTS=$(jq -c --arg k "$1" --arg b "$2" --arg r "$3" --argjson kw "$4" \
    '.[$k]={blobId:$b,mailboxIds:{m1:true},keywords:$kw,receivedAt:$r}' <<<"$IMPORTS")
}
BOB="Bob <bob@example.com>"
add e1 "$(blob 'Alice Park <alice@example.com>' "$BOB" '' 'Quarterly report' q2-report@example.com '' 'Mon, 01 Jun 2026 09:00:00 +0000' 'Q2 revenue grew 12% to $4.2M. Churn fell to 2.1%.')" 2026-06-01T09:00:00Z '{}'
add e2 "$(blob 'Alice Park <alice@example.com>' "$BOB" '' 'Lunch Friday?' lunch-1@example.com '' 'Tue, 02 Jun 2026 10:00:00 +0000' 'Are you free for lunch on Friday at noon?')" 2026-06-02T10:00:00Z '{"$seen":true}'
add e3 "$(blob 'Alice Park <alice@example.com>' "$BOB" '' 'Re: Lunch Friday?' lunch-2@example.com lunch-1@example.com 'Tue, 02 Jun 2026 11:00:00 +0000' 'Noon works. See you at the cafe on Pine Street.')" 2026-06-02T11:00:00Z '{}'
add e4 "$(blob 'Acme Billing <billing@acme-supplies.example>' "$BOB" '' 'Invoice 8841' inv-1@acme-supplies.example '' 'Wed, 03 Jun 2026 08:30:00 +0000' 'Invoice 8841: amount due $1,284.50, payable by 2026-07-15.')" 2026-06-03T08:30:00Z '{"$seen":true,"$flagged":true}'
add e5 "$(blob 'Acme Billing <billing@acme-supplies.example>' "$BOB" '' 'Re: Invoice 8841' inv-2@acme-supplies.example inv-1@acme-supplies.example 'Thu, 11 Jun 2026 08:30:00 +0000' 'Reminder: invoice 8841 is still open. Late fee of $25 applies after 2026-07-15.')" 2026-06-11T08:30:00Z '{}'
add e6 "$(blob 'Rust Meetup <events@rustmeetup.example>' "$BOB" '' 'Next meetup: async Rust' meetup-1@rustmeetup.example '' 'Fri, 05 Jun 2026 12:00:00 +0000' 'Join us on 2026-06-18 at 18:30 in room Cedar. Speaker: Dana Okafor.')" 2026-06-05T12:00:00Z '{"$seen":true}'
add e7 "$(blob 'SkyAir <travel@skyair.example>' "$BOB" '' 'Flight confirmation QX7R2L' flight-1@skyair.example '' 'Sat, 06 Jun 2026 15:00:00 +0000' 'Booking reference QX7R2L. Departs 2026-08-03 at 07:45 from PDX, arrives 10:20 in DEN.')" 2026-06-06T15:00:00Z '{"$seen":true}'
add e8 "$(blob 'Weekly News <news@weekly.example>' "$BOB" '' 'This week in tech' news-1@weekly.example '' 'Sun, 07 Jun 2026 06:00:00 +0000' 'Ten stories you may have missed this week.')" 2026-06-07T06:00:00Z '{}'
add e9 "$(blob 'Ops Alerts <ops@bob-corp.example>' "$BOB" '' 'ALERT: disk usage' alert-1@bob-corp.example '' 'Mon, 08 Jun 2026 02:15:00 +0000' 'Disk usage is at 91% on host db-02. Threshold is 85%.')" 2026-06-08T02:15:00Z '{}'
add e10 "$(blob 'Carol Reyes <carol@example.com>' "$BOB" 'Dave Lin <dave@example.com>' 'Project Orion kickoff' orion-1@example.com '' 'Mon, 09 Jun 2026 09:00:00 +0000' 'Kickoff is Tuesday at 10:00. The approved budget is $85k.')" 2026-06-09T09:00:00Z '{}'
add e11 "$(blob 'Dave Lin <dave@example.com>' "$BOB, Carol Reyes <carol@example.com>" '' 'Re: Project Orion kickoff' orion-2@example.com orion-1@example.com 'Mon, 09 Jun 2026 11:30:00 +0000' 'Scope change: we are dropping the mobile app from phase one.')" 2026-06-09T11:30:00Z '{}'
jmap "$(jq -nc --argjson e "$IMPORTS" '{using:["urn:ietf:params:jmap:core","urn:ietf:params:jmap:mail"],methodCalls:[["Email/import",{accountId:"a1",emails:$e},"i"]]}')" \
  | jq -e '.methodResponses[0][1].notCreated == null' >/dev/null || { echo "seed failed" >&2; exit 1; }

echo "export JMAP_URL=$URL JMAP_TOKEN=$TOKEN JMAP_DEV_DIR=$D"
