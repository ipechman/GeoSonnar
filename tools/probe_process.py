"""Small Windows ownership probe; no compiler, GUI, network or CPU benchmark.

python tools/probe_process.py --output output/validation/job-policy-001
The observer terminates its exact launcher and verifies the owned worker/leaf
exit while an unrelated, idle sibling survives. All children are hidden.
"""
import argparse
import ctypes as c
from ctypes import wintypes as w
import os
from pathlib import Path
import subprocess
import sys
import time

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools import evidence as e, process
from tools.validate import fresh_directory


def child(mode, output):
    if mode == "owner":
        process.run([sys.executable, "-B", __file__, "--mode", "worker", "--output", output],
                    cwd=e.ROOT, log_prefix=output / "worker", aggregate=True, timeout_s=30)
    elif mode == "worker":
        e.write(output / "worker-ready.json", {"pid": os.getpid(), "aggregate": process.current_job()}, replace=True)
        process.run([sys.executable, "-B", __file__, "--mode", "leaf", "--output", output],
                    cwd=e.ROOT, log_prefix=output / "leaf", timeout_s=25)
    elif mode == "leaf":
        api = process._Windows()
        proof = {"schema_version": 2, "policy": "inherited_aggregate_no_additional_cpu_cap",
                 "aggregate": process.current_job(), "owned_job": api.query(None),
                 "aggregate_membership_queried": True,
                 "owned_membership_queried": api.contains(api.kernel.GetCurrentProcess(), None),
                 "below_normal_priority_queried": api.kernel.GetPriorityClass(api.kernel.GetCurrentProcess()) == 0x4000}
        process.verify_command_proof(proof)
        e.write(output / "leaf-ready.json", {"pid": os.getpid(), "job": proof}, replace=True)
        time.sleep(25)
    elif mode == "sibling":
        e.write(output / "sibling-ready.json", {"pid": os.getpid()}, replace=True)
        time.sleep(25)


def probe(output):
    e.require(os.name == "nt", "Windows JobObject probe requires Windows")
    output = fresh_directory(output)
    bound = [e.receipt(Path(__file__).resolve()), e.receipt(Path(process.__file__).resolve()), e.receipt(sys.executable)]
    proof = {"schema_version": 2, "status": "failed", "inputs": bound,
             "scope": "Idle ownership/policy controls only; no throughput or runtime claim",
             "unrelated_sibling_idle_outside_tested_job": True}
    owner, sibling, handles = None, None, []
    kernel = c.WinDLL("kernel32", use_last_error=True)
    for name, arguments, result in [
        ("OpenProcess", [w.DWORD, w.BOOL, w.DWORD], w.HANDLE),
        ("WaitForSingleObject", [w.HANDLE, w.DWORD], w.DWORD),
        ("TerminateProcess", [w.HANDLE, w.UINT], w.BOOL),
        ("CloseHandle", [w.HANDLE], w.BOOL),
    ]:
        method = getattr(kernel, name); method.argtypes = arguments; method.restype = result
    began = time.monotonic()
    def launch(mode):
        return subprocess.Popen([sys.executable, "-B", __file__, "--mode", mode, "--output", str(output)],
            cwd=e.ROOT, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            creationflags=subprocess.CREATE_NO_WINDOW | subprocess.BELOW_NORMAL_PRIORITY_CLASS)
    try:
        sibling, owner = launch("sibling"), launch("owner")
        expected = [output / (name + "-ready.json") for name in ("worker", "leaf", "sibling")]
        while not all(path.is_file() for path in expected):
            e.require(owner.poll() is None and sibling.poll() is None, "Probe child exited before readiness; inspect retained worker logs")
            e.require(time.monotonic() - began < 15, "Probe readiness deadline exceeded")
            time.sleep(0.02)
        worker, leaf, sibling_ready = [e.read(path, 8192) for path in expected]
        process.verify_command_proof(leaf["job"], worker["aggregate"])
        e.require(sibling_ready["pid"] == sibling.pid, "Sibling identity differs")
        for row in (worker, leaf):
            handle = kernel.OpenProcess(0x00100000 | 0x0001, False, row["pid"])
            if not handle: raise c.WinError(c.get_last_error())
            handles.append(handle)
            e.require(kernel.WaitForSingleObject(handle, 0) == 258, "Owned child was not alive before launcher exit")
        owner.terminate(); owner.wait(timeout=5)
        for handle in handles:
            e.require(kernel.WaitForSingleObject(handle, 5000) == 0, "Launcher exit did not terminate its owned tree")
        e.require(sibling.poll() is None, "Unrelated sibling was incorrectly terminated")
        for item in bound: e.verify(item)
        proof.update(status="passed", queried_command_policy=leaf["job"],
                     launcher_exit_killed_worker_and_descendant=True, unrelated_sibling_survived=True,
                     readiness=[e.receipt(path, output) for path in expected])
    except BaseException as error:
        proof["error"] = type(error).__name__ + ": " + str(error)
    finally:
        for child_process in (owner, sibling):
            if child_process is not None and child_process.poll() is None:
                child_process.terminate(); child_process.wait(timeout=5)
        for handle in handles:
            if kernel.WaitForSingleObject(handle, 0) == 258:
                kernel.TerminateProcess(handle, 1); kernel.WaitForSingleObject(handle, 5000)
            kernel.CloseHandle(handle)
        proof["elapsed_s"] = time.monotonic() - began
        e.write(output / "probe.json", proof)
    e.require(proof["status"] == "passed", proof.get("error", "Windows ownership probe failed"))
    print(str(output / "probe.json"))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--mode", choices=["owner", "worker", "leaf", "sibling"], help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.mode: child(args.mode, args.output)
    else: probe(args.output)
