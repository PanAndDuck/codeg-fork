#!/usr/bin/env python3
"""Own exactly the local E0 processes and disposable data recorded in state."""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time

from harness import ROOT, STATE, api, load_state, state_path

REPO = ROOT.parent
RELAY = REPO / "crates/codeg-relay/target/release/codeg-relay"
SERVER = REPO / "src-tauri/target/debug/codeg-server"
WRAPPER = REPO / "scripts/fork-server-dev.sh"


def clean_env():
    env = {k: v for k, v in os.environ.items() if not k.startswith("CODEG")}
    for key in ("TMPDIR", "TMP", "TEMP"):
        env.pop(key, None)
    env["PATH"] = f"/opt/homebrew/opt/rustup/bin:{ROOT / 'mock-agent'}:" + env.get("PATH", "")
    env["CARGO_BUILD_JOBS"] = "4"
    env["CARGO_PROFILE_DEV_DEBUG"] = "0"
    return env


def official_check():
    command = subprocess.check_output(["ps", "-p", "87598", "-o", "command="], text=True).strip()
    output = subprocess.check_output(["lsof", "-nP", "-iTCP:3080", "-sTCP:LISTEN", "-t"], text=True)
    if command != "/Applications/codeg.app/Contents/MacOS/codeg" or set(output.split()) != {"87598"}:
        raise RuntimeError("official process/3080 baseline changed; refusing lifecycle action")
    print("official: PID 87598 /Applications/codeg.app/Contents/MacOS/codeg; sole 3080 listener", flush=True)


def identity(pid):
    result = subprocess.run(["ps", "-p", str(pid), "-o", "lstart=,command="], text=True, capture_output=True)
    return result.stdout.strip() if result.returncode == 0 else None


def record(pid, expected):
    value = identity(pid)
    if not value or str(expected) not in value:
        raise RuntimeError(f"PID {pid} does not match owned executable {expected}")
    return {"pid": pid, "identity": value, "expected": str(expected)}


def stop(proc):
    pid = proc["pid"]
    value = identity(pid)
    if value is None:
        return
    if value != proc["identity"] or proc["expected"] not in value or pid == 87598:
        raise RuntimeError(f"refusing to stop PID {pid}: process identity changed")
    os.kill(pid, signal.SIGTERM)
    for _ in range(100):
        try:
            os.waitpid(pid, os.WNOHANG)
        except ChildProcessError:
            pass
        if identity(pid) is None:
            return
        time.sleep(.1)
    if identity(pid) == proc["identity"]:
        os.kill(pid, signal.SIGKILL)
        for _ in range(30):
            try:
                os.waitpid(pid, os.WNOHANG)
            except ChildProcessError:
                pass
            if identity(pid) is None:
                return
            time.sleep(.1)
    raise RuntimeError(f"owned PID {pid} did not exit")


def save(state):
    path = state_path(state["name"])
    atomic_write(path, json.dumps(state, indent=2) + "\n")
    fields = {"E2E_NAME": state["name"], "E2E_ROOT": state["temp_root"],
              "E2E_RELAY_PORT": state["relay_port"], "E2E_SERVER_PORT": state["server_port"],
              "E2E_PROXY_PORT": state["proxy_port"], "E2E_CONTROL_PORT": state["control_port"]}
    for key in ("token", "fingerprint", "proxy_fingerprint", "relay_url", "server_url", "proxy_url",
                "data_dir", "home_dir", "agent_key_file", "relay_dir", "proxy_mode", "server_log"):
        if key in state:
            fields["E2E_" + key.upper()] = state[key]
    for role, proc in state["processes"].items():
        fields["E2E_" + role.upper() + "_PID"] = proc["pid"]
    envfile = path.with_suffix(".env")
    atomic_write(envfile, "# Private disposable credentials; do not commit.\n" +
                 "".join(f"{k}={shlex.quote(str(v))}\n" for k, v in fields.items()))


def atomic_write(path, content):
    fd, temporary = tempfile.mkstemp(prefix=path.name + ".", dir=path.parent)
    try:
        with os.fdopen(fd, "w") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)


def free_ports(state):
    ports = [state[k] for k in ("relay_port", "server_port", "proxy_port", "control_port")]
    if len(set(ports)) != 4 or not 3090 <= state["server_port"] <= 3099 or not 18445 <= state["control_port"] <= 18499:
        raise ValueError("distinct ports required: server 3090–3099; control 18445–18499")
    for port in (state["relay_port"], state["proxy_port"]):
        if not 18400 <= port <= 18499:
            raise ValueError("relay/proxy ports must be 18400–18499")
    for port in ports:
        with socket.socket() as sock:
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            sock.bind(("127.0.0.1", port))


def wait_for(fn, timeout, label):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            result = fn()
            if result:
                return result
        except (OSError, ValueError, RuntimeError) as exc:
            last = type(exc).__name__
        time.sleep(.2)
    raise RuntimeError(f"timeout waiting for {label} ({last}); inspect private state logs")


def spawn(state, role, args, env, log):
    env.update(TMPDIR=state["temp_root"], TMP=state["temp_root"], TEMP=state["temp_root"])
    with open(log, "ab") as output:
        child = subprocess.Popen(args, cwd=REPO, env=env, stdin=subprocess.DEVNULL,
                                 stdout=output, stderr=output, start_new_session=True)
    expected = args[0] if role != "proxy" else str(ROOT / "faultproxy/faultproxy.py")
    proc = None
    try:
        proc = record(child.pid, expected)
        state["processes"][role] = proc
        save(state)
    except BaseException:
        if proc:
            stop(proc)
        elif child.poll() is None:
            # Do not fall back to an unverified kill if exec/identity failed.
            raise RuntimeError(f"could not verify spawned PID {child.pid}; expected {expected}")
        raise
    return child


def relay_start(state):
    env = clean_env()
    env.update(CODEG_DATA_DIR=state["relay_dir"], CODEG_HOME=str(Path(state["temp_root"]) / "relay-home"))
    spawn(state, "relay", [str(RELAY), "serve", "--listen", f"127.0.0.1:{state['relay_port']}",
        "--agent-key-file", state["agent_key_file"], "--self-signed", "--public-host", "127.0.0.1,localhost",
        "--data-dir", state["relay_dir"]], env, Path(state["temp_root"]) / "relay.log")


def up(args):
    path = state_path(args.name)
    STATE.mkdir(mode=0o700, exist_ok=True)
    if path.exists():
        raise ValueError("environment already exists; use env-down first")
    state = {"name": args.name, "repo": str(REPO), "processes": {}, "relay_port": args.relay_port,
             "server_port": args.server_port, "proxy_port": args.proxy_port, "control_port": args.control_port,
             "proxy_mode": args.proxy_mode}
    free_ports(state)
    # The official ACP launcher also supplies TMPDIR. Never nest our state
    # under its scratch tree, even though the seven CODEG variables are clear.
    temp = Path(tempfile.mkdtemp(prefix=f"codeg-e2e-{args.name}-", dir="/tmp")).resolve()
    state.update(temp_root=str(temp), relay_dir=str(temp / "relay"), agent_key_file=str(temp / "agent.key"),
                 relay_url=f"https://127.0.0.1:{args.relay_port}",
                 server_url=f"http://127.0.0.1:{args.server_port}", proxy_url=f"https://127.0.0.1:{args.proxy_port}")
    try:
        (temp / ".e2e-owner").write_text(str(REPO) + "\n" + args.name)
        save(state)
    except BaseException:
        shutil.rmtree(temp)
        path.unlink(missing_ok=True)
        path.with_suffix(".env").unlink(missing_ok=True)
        raise
    try:
        env = clean_env()
        env.update(CODEG_DATA_DIR=str(temp / "build-data"), CODEG_HOME=str(temp / "build-home"),
                   CARGO_TARGET_DIR=str(RELAY.parents[1]), TMPDIR=str(temp))
        if not args.skip_build:
            print("building release relay (e2e/.state/build-relay.log)", flush=True)
            with open(STATE / "build-relay.log", "w") as log:
                subprocess.run(["cargo", "build", "--release", "--manifest-path", "crates/codeg-relay/Cargo.toml"],
                               cwd=REPO, env=env, stdout=log, stderr=log, check=True)
        if not RELAY.is_file():
            raise ValueError("local relay binary missing; omit --skip-build")
        key = subprocess.check_output([str(RELAY), "gen-key"], env=env, text=True).strip()
        Path(state["agent_key_file"]).write_text(key)
        Path(state["agent_key_file"]).chmod(0o600)
        relay_start(state)
        cert = Path(state["relay_dir"]) / "tls/cert.pem"
        wait_for(lambda: cert.exists(), 15, "relay certificate")
        state["fingerprint"] = subprocess.check_output([str(RELAY), "fingerprint", "--data-dir", state["relay_dir"]],
                                                        env=env, text=True).splitlines()[0]
        env = clean_env()
        env.update(CODEG_DATA_DIR=str(temp / "wrapper-data"), CODEG_HOME=str(temp / "wrapper-home"),
                   CODEG_PORT=str(args.server_port), CODEG_RELAY_URL=state["relay_url"], CODEG_RELAY_KEY=key,
                   CODEG_RELAY_FINGERPRINT=state["fingerprint"], CODEG_RELAY_PUBLIC_URL=state["relay_url"],
                   CODEG_RELAY_ENABLED="true", TMPDIR=str(temp))
        (REPO / "out").mkdir(exist_ok=True)  # Cargo resource existence, not a frontend build.
        state["server_log"] = str(temp / "server.log")
        print("building/starting fork server via scripts/fork-server-dev.sh", flush=True)
        wrapper = spawn(state, "wrapper", [str(WRAPPER)], env, state["server_log"])

        def wrapper_ready():
            log = Path(state["server_log"]).read_text(errors="replace")
            match = re.search(r"^PID: (\d+)$", log, re.M)
            if match:
                for label, keyname in (("Token", "token"), ("Home", "home_dir"), ("Data", "data_dir")):
                    state[keyname] = re.search(rf"^{label}: (.+)$", log, re.M)[1]
                state["processes"]["server"] = record(int(match[1]), SERVER)
                save(state)
                return True
            if wrapper.poll() is not None:
                raise subprocess.CalledProcessError(wrapper.returncode, [str(WRAPPER)])
            return False
        wait_for(wrapper_ready, 1800, "fork build/start")
        wait_for(lambda: api(state["server_url"], state["token"], "get_relay_status", timeout=2)["state"] == "connected",
                 40, "relay connected")
        if not args.no_proxy:
            proxy_start(state)
        save(state)
        print(f"connected; state={path.with_suffix('.env')} (0600)", flush=True)
        print(f"relay={state['relay_url']} proxy={state['proxy_url']} mode={state['proxy_mode']}", flush=True)
    except BaseException:
        down_state(state)
        raise


def proxy_start(state):
    temp = Path(state["temp_root"])
    env = clean_env()
    env.update(CODEG_DATA_DIR=str(temp / "proxy-data"), CODEG_HOME=str(temp / "proxy-home"))
    args = [sys.executable, str(ROOT / "faultproxy/faultproxy.py"), "--mode", state["proxy_mode"],
            "--port", str(state["proxy_port"]), "--control-port", str(state["control_port"]),
            "--upstream-port", str(state["relay_port"]), "--upstream-fingerprint", state["fingerprint"]]
    state["proxy_fingerprint"] = state["fingerprint"]
    if state["proxy_mode"] == "terminate":
        # Python ssl cannot mint certificates; the existing system openssl is
        # used only to create this disposable test identity (no Python packages).
        cert, key = temp / "proxy-cert.pem", temp / "proxy-key.pem"
        subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2",
                        "-subj", "/CN=codeg-e2e-proxy", "-keyout", str(key), "-out", str(cert)],
                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
        key.chmod(0o600)
        import ssl
        state["proxy_fingerprint"] = hashlib.sha256(ssl.PEM_cert_to_DER_cert(cert.read_text())).hexdigest()
        args += ["--cert", str(cert), "--key", str(key)]
    spawn(state, "proxy", args, env, temp / "proxy.log")
    wait_for(lambda: api(f"http://127.0.0.1:{state['control_port']}", "", "/status", timeout=1), 10, "fault proxy")


def down_state(state):
    # Include build children if startup is interrupted before the wrapper can
    # print the server PID. Ownership comes from a verified root's ancestry;
    # every descendant's full command + start time is checked again at stop.
    owned = {p["pid"] for p in state["processes"].values() if identity(p["pid"]) == p["identity"]}
    roots = set(owned)
    children = []
    output = subprocess.check_output(["ps", "-axo", "pid=,ppid=,command="], text=True)
    rows = [line.split(None, 2) for line in output.splitlines()]
    while True:
        added = False
        for parts in rows:
            if len(parts) != 3:
                continue
            pid, parent, command = int(parts[0]), int(parts[1]), parts[2]
            if parent in owned and pid not in owned and pid != 87598:
                owned.add(pid)
                added = True
                if identity(pid):
                    children.append(record(pid, command))
        if not added:
            break
    for role in ("proxy", "server", "wrapper", "relay"):
        if role in state["processes"]:
            stop(state["processes"][role])
    for child in reversed(children):
        stop(child)
    temp = Path(state["temp_root"])
    marker = temp / ".e2e-owner"
    if (temp.is_symlink() or temp.resolve().parent != Path("/tmp").resolve()
            or not temp.name.startswith("codeg-e2e-" + state["name"] + "-")
            or marker.read_text() != str(REPO) + "\n" + state["name"]):
        raise RuntimeError("refusing to remove unowned temporary directory")
    shutil.rmtree(temp)
    free_ports(state)
    path = state_path(state["name"])
    path.unlink(missing_ok=True)
    path.with_suffix(".env").unlink(missing_ok=True)
    path.with_suffix(".session.json").unlink(missing_ok=True)
    print(f"stopped owned PIDs {sorted(roots | owned)}; removed disposable data; all four test ports free", flush=True)


def main():
    os.umask(0o077)
    # SIGTERM during startup follows the same rollback path as Ctrl-C.
    def interrupted(signum, frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("up", "down", "restart-relay", "restart-proxy"))
    parser.add_argument("--name", required=True)
    parser.add_argument("--relay-port", type=int, default=18443)
    parser.add_argument("--server-port", type=int, default=3091)
    parser.add_argument("--proxy-port", type=int, default=18444)
    parser.add_argument("--control-port", type=int, default=18445)
    parser.add_argument("--proxy-mode", choices=("passthrough", "terminate"), default="terminate")
    parser.add_argument("--no-proxy", action="store_true")
    parser.add_argument("--skip-build", action="store_true", help="reuse relay binary; C1 wrapper still checks server build")
    parser.add_argument("--downtime", type=float, default=10)
    args = parser.parse_args()
    STATE.mkdir(mode=0o700, exist_ok=True)
    lock = open(state_path(args.name).with_suffix(".lock"), "w")
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    official_check()
    try:
        if args.action == "up":
            up(args)
        elif args.action == "down":
            down_state(load_state(args.name))
        elif args.action == "restart-proxy":
            state = load_state(args.name)
            if "proxy" in state["processes"]:
                stop(state["processes"]["proxy"])
            state["proxy_mode"] = args.proxy_mode
            proxy_start(state)
            save(state)
            print("proxy ready: " + state["proxy_mode"], flush=True)
        else:
            state = load_state(args.name)
            if not 0 <= args.downtime <= 60:
                raise ValueError("downtime must be 0–60 seconds")
            stop(state["processes"]["relay"])
            time.sleep(args.downtime)
            relay_start(state)
            wait_for(lambda: api(state["server_url"], state["token"], "get_relay_status", timeout=2)["state"] == "connected",
                     45, "restarted relay")
            print("relay restarted with same key/certificate; connected", flush=True)
    finally:
        official_check()


if __name__ == "__main__":
    main()
