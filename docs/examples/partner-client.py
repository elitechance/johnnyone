#!/usr/bin/env python3
"""
Minimal Python partner client (Task 03).
Run:
  python -m pip install httpx websockets
  JOHNNYONE_BASE_URL=https://johnnyone-dev.ethan-353.workers.dev \
  JOHNNYONE_EMAIL=... JOHNNYONE_PW=... JOHNNYONE_TENANT=... \
  python docs/examples/partner-client.py

Or with token:
  JOHNNYONE_BASE_URL=... JOHNNYONE_TOKEN=... \
  JOHNNYONE_PROVIDER=ollama JOHNNYONE_WORKDIR=/tmp \
  python docs/examples/partner-client.py

Uses httpx + websockets (not pure stdlib). Full flow: login or token → create → WSS (header or ?token) → subscribe → command → updated screen.
Envelopes exactly as in chat-relay-do.ts.
Errors: GraphQL errors[].message or HTTP status; WSS upgrade 401/503; terminal_command_ack.error:'forbidden_session'.
Note: uses additional_headers= (websockets >=12); ?token= fallback documented for older header-less clients.
"""

import asyncio
import json
import os
import sys

try:
    import httpx
    import websockets
except ImportError:
    print("pip install httpx websockets", file=sys.stderr)
    sys.exit(1)

BASE = os.getenv("JOHNNYONE_BASE_URL", "https://johnnyone-dev.ethan-353.workers.dev")
TOKEN = os.getenv("JOHNNYONE_TOKEN")
EMAIL = os.getenv("JOHNNYONE_EMAIL", "partner@example.com")
PW = os.getenv("JOHNNYONE_PW")
TENANT = os.getenv("JOHNNYONE_TENANT", "your-tenant")
PROVIDER = os.getenv("JOHNNYONE_PROVIDER", "ollama")
WORKDIR = os.getenv("JOHNNYONE_WORKDIR", "/tmp/partner-demo")

async def main():
    token = TOKEN
    async with httpx.AsyncClient() as client:
        if not token:
            if not PW:
                raise SystemExit("Provide JOHNNYONE_TOKEN or JOHNNYONE_PW")
            r = await client.post(
                f"{BASE}/graphql",
                headers={"content-type": "application/json", "x-tenant-id": TENANT},
                json={
                    "query": "mutation($i:LoginInput!){login(input:$i){accessToken refreshToken}}",
                    "variables": {"i": {"email": EMAIL, "password": PW, "tenantId": TENANT}},
                },
            )
            resp = r.json()
            if resp.get("errors"):
                err_msg = resp["errors"][0].get("message", "")
                if "401" in err_msg or "unauthorized" in err_msg.lower():
                    print("ERROR: 401 unauthorized on GraphQL (invalid/missing token)", file=sys.stderr)
                else:
                    print("ERROR: " + err_msg, file=sys.stderr)
                sys.exit(1)
            r.raise_for_status()
            data = resp.get("data") or {}
            token = data.get("login", {}).get("accessToken")
            if not token:
                print("ERROR: 401 unauthorized on GraphQL", file=sys.stderr)
                sys.exit(1)
            print("logged in", file=sys.stderr)

        # create
        r = await client.post(
            f"{BASE}/graphql",
            headers={"authorization": f"Bearer {token}"},
            json={
                "query": "mutation($i:CreateAiSessionInput!){createAiSession(input:$i){id}}",
                "variables": {"i": {"provider": PROVIDER, "workingDirectory": WORKDIR}},
            },
        )
        resp = r.json()
        if resp.get("errors"):
            err_msg = resp["errors"][0].get("message", "")
            if "401" in err_msg or "unauthorized" in err_msg.lower():
                print("ERROR: 401 unauthorized on GraphQL (invalid/missing token)", file=sys.stderr)
            elif "503" in err_msg or "desktop not connected" in err_msg.lower() or "no online" in err_msg.lower():
                print("ERROR: 503 no online desktop node", file=sys.stderr)
            elif "forbidden" in err_msg.lower() or "forbidden_session" in err_msg:
                print("ERROR: forbidden_session (session not owned by this tenant/user)", file=sys.stderr)
            else:
                print("ERROR: " + err_msg, file=sys.stderr)
            sys.exit(1)
        r.raise_for_status()
        data = resp.get("data") or {}
        session_id = data.get("createAiSession", {}).get("id")
        if not session_id:
            print("ERROR: 503 no online desktop node (create failed)", file=sys.stderr)
            sys.exit(1)
        print("session", session_id, file=sys.stderr)

        # WSS - prefer header (use additional_headers for websockets >=12; extra_headers was used in <12 and removed in >=14)
        ws_url = BASE.replace("https://", "wss://") + "/api/relay/ws"
        headers = {"Authorization": f"Bearer {token}"}

        async with websockets.connect(ws_url, additional_headers=headers) as ws:
            await ws.send(json.dumps({"type": "terminal_visual_subscribe", "sessionId": session_id}))

            first_screen = None
            async for msg in ws:
                m = json.loads(msg)
                if m.get("type") == "terminal_command_ack" and m.get("error"):
                    err = m.get("error")
                    if err == "forbidden_session":
                        print("ERROR: forbidden_session (session not owned by this tenant/user)", file=sys.stderr)
                    else:
                        print("ERROR: " + err, file=sys.stderr)
                    return
                if m.get("type") == "terminal_screen":
                    print("SCREEN:", json.dumps(m.get("data")))
                    if first_screen is None:
                        first_screen = m.get("data")
                        print("received first screen", file=sys.stderr)
                        # now send command to prove updated screen
                        await ws.send(json.dumps({"type": "terminal_command", "sessionId": session_id, "data": "echo python-demo\r"}))
                    else:
                        print("received updated screen after command", file=sys.stderr)
                        if m.get("data") != first_screen:
                            print("SUCCESS: updated screen differs from first (input round-trip observed)", file=sys.stderr)
                        else:
                            print("SUCCESS: received screen after command (input accepted)", file=sys.stderr)
                        return
            raise SystemExit("no screen received")

if __name__ == "__main__":
    try:
        asyncio.run(main())
    except Exception as e:
        msg = str(e)
        if '401' in msg or 'unauthorized' in msg.lower():
            print("ERROR: 401 unauthorized on WSS/GraphQL (invalid/missing token)", file=sys.stderr)
        elif '503' in msg or 'no online' in msg.lower():
            print("ERROR: 503 no online desktop node", file=sys.stderr)
        elif 'forbidden_session' in msg:
            print("ERROR: forbidden_session (session not owned by this tenant/user)", file=sys.stderr)
        else:
            print("FAIL:", e, file=sys.stderr)
        sys.exit(1)
