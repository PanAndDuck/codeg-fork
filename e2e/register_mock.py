#!/usr/bin/env python3
"""Register/enable the local mock through the isolated server's own APIs."""
import argparse
import json
from harness import api, endpoint, load_state, state_path

AGENT_TYPE = "custom:e2e-mock"


def register(state, through="relay", connect=False, settle_ms=100):
    if not 0 <= settle_ms <= 10000:
        raise ValueError("settle_ms must be 0–10000")
    url, pin = endpoint(state, through)

    def call(command, params=None):
        return api(url, state["token"], command, params, pin)

    platform = call("acp_current_platform")
    call("acp_save_custom_agent", {"params": {"registryId": "e2e-mock", "name": "E2E scripted mock",
        "description": "Local deterministic test agent; no LLM/network calls", "version": "1.0.0",
        "source": "manual", "supportsMcp": False, "skillsSharedStore": False,
        "distributionKind": "binary", "spec": {"binary": {platform: {
            "archive": "https://example.invalid/e2e-mock-never-download.tar.gz",
            "cmd": "mock_acp_agent.py", "args": ["--settle-ms", str(settle_ms)], "env": {}}}}}})
    call("acp_update_agent_env", {"agentType": AGENT_TYPE, "enabled": True, "env": {}})
    directory = call("create_chat_dir")["path"]
    created = call("create_chat_conversation", {"agentType": AGENT_TYPE,
                   "title": "E0 deterministic smoke", "existingDir": directory})
    result = {"agentType": AGENT_TYPE, "workingDir": directory,
              "folderId": created["folderId"], "conversationId": created["conversationId"]}
    if connect:
        result["connectionId"] = call("acp_connect", {"agentType": AGENT_TYPE,
                                              "workingDir": directory, "sessionId": None})
    target = state_path(state["name"]).with_suffix(".session.json")
    target.write_text(json.dumps(result, indent=2) + "\n")
    target.chmod(0o600)
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name", required=True)
    parser.add_argument("--through", choices=("relay", "proxy", "server"), default="relay")
    parser.add_argument("--connect", action="store_true")
    parser.add_argument("--settle-ms", type=int, default=100)
    args = parser.parse_args()
    print(json.dumps(register(load_state(args.name), args.through, args.connect, args.settle_ms), indent=2))
