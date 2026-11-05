"""Owned Windows children: hidden, below-normal, 20% aggregate CPU, bounded logs.

Only our suspended child is assigned; closing its JobObject terminates descendants.
No process-name killing, visible shell, global environment edit or ignored helper.
The ownership implementation follows the existing native validation runner.
"""
import os
from pathlib import Path
import subprocess
import threading
import time
import uuid
from tools.evidence import receipt, require, write


AGGREGATE_JOB_ENV = "GEOSONNAR_VALIDATION_AGGREGATE_JOB"


def verify_launcher_host(proof):
    require(proof.get("queried") is True and proof.get("immediate_job_flags") == 0 and
            proof.get("ancestor_policies_unknown") is True and proof.get("external_limits_may_reduce_availability") is True,
            "Immediate host job has CPU rate control; do not stack a validation cap inside it")


def verify_aggregate_proof(proof):
    require(proof.get("schema_version") == 2 and proof.get("policy") == "aggregate_20_percent" and
            proof.get("job_flags") == 5 and proof.get("job_rate") == 2000 and proof.get("cpu_percent") == 20 and
            proof.get("queried") is True and proof.get("kill_on_close") is True and
            isinstance(proof.get("job_name"), str) and proof["job_name"].startswith("Local\\GeoSonnarValidation-"),
            "Missing queried aggregate 20% CPU policy")


def verify_command_proof(proof, aggregate=None):
    require(proof.get("schema_version") == 2 and proof.get("policy") == "inherited_aggregate_no_additional_cpu_cap",
            "Command must inherit the aggregate cap without adding another CPU quota")
    verify_aggregate_proof(proof.get("aggregate", {}))
    if aggregate is not None:
        verify_aggregate_proof(aggregate)
        require(proof["aggregate"] == aggregate, "Command belongs to a different aggregate job")
    require(proof.get("owned_job") == {"job_flags": 0, "job_rate": 0, "queried": True, "kill_on_close": True} and
            proof.get("aggregate_membership_queried") is True and proof.get("owned_membership_queried") is True and
            proof.get("below_normal_priority_queried") is True,
            "Owned command job/membership/priority proof differs")


class _Windows:
    def __init__(self):
        require(os.name == "nt", "Capped execution currently requires Windows; no uncapped fallback")
        import ctypes as c
        from ctypes import wintypes as w
        self.c, self.kernel = c, c.WinDLL("kernel32", use_last_error=True)
        class Basic(c.Structure):
            _fields_ = [("process_time", c.c_int64), ("job_time", c.c_int64), ("flags", w.DWORD),
                        ("min_ws", c.c_size_t), ("max_ws", c.c_size_t), ("process_limit", w.DWORD),
                        ("affinity", c.c_size_t), ("priority", w.DWORD), ("scheduling", w.DWORD)]
        class Io(c.Structure):
            _fields_ = [(name, c.c_uint64) for name in ("read_ops", "write_ops", "other_ops", "read_bytes", "write_bytes", "other_bytes")]
        class Limits(c.Structure):
            _fields_ = [("basic", Basic), ("io", Io), ("process_memory", c.c_size_t),
                        ("job_memory", c.c_size_t), ("peak_process", c.c_size_t), ("peak_job", c.c_size_t)]
        class Rate(c.Structure):
            _fields_ = [("flags", w.DWORD), ("rate", w.DWORD)]
        class Thread(c.Structure):
            _fields_ = [("size", w.DWORD), ("usage", w.DWORD), ("thread_id", w.DWORD),
                        ("process_id", w.DWORD), ("priority", w.LONG), ("delta", w.LONG), ("flags", w.DWORD)]
        self.Thread, self.Rate, self.Limits = Thread, Rate, Limits
        signatures = {
            "CreateJobObjectW": ([c.c_void_p, w.LPCWSTR], w.HANDLE),
            "OpenJobObjectW": ([w.DWORD, w.BOOL, w.LPCWSTR], w.HANDLE),
            "GetCurrentProcess": ([], w.HANDLE),
            "IsProcessInJob": ([w.HANDLE, w.HANDLE, c.POINTER(w.BOOL)], w.BOOL),
            "GetPriorityClass": ([w.HANDLE], w.DWORD),
            "SetInformationJobObject": ([w.HANDLE, c.c_int, c.c_void_p, w.DWORD], w.BOOL),
            "QueryInformationJobObject": ([w.HANDLE, c.c_int, c.c_void_p, w.DWORD, c.c_void_p], w.BOOL),
            "AssignProcessToJobObject": ([w.HANDLE, w.HANDLE], w.BOOL),
            "SetPriorityClass": ([w.HANDLE, w.DWORD], w.BOOL), "CloseHandle": ([w.HANDLE], w.BOOL),
            "CreateToolhelp32Snapshot": ([w.DWORD, w.DWORD], w.HANDLE),
            "Thread32First": ([w.HANDLE, c.POINTER(Thread)], w.BOOL),
            "Thread32Next": ([w.HANDLE, c.POINTER(Thread)], w.BOOL),
            "OpenThread": ([w.DWORD, w.BOOL, w.DWORD], w.HANDLE), "ResumeThread": ([w.HANDLE], w.DWORD)}
        for name, (arguments, result) in signatures.items():
            function = getattr(self.kernel, name); function.argtypes = arguments; function.restype = result

    def query(self, handle):
        c = self.c
        rate, limits = self.Rate(), self.Limits()
        for kind, value in ((15, rate), (9, limits)):
            if not self.kernel.QueryInformationJobObject(handle, kind, c.byref(value), c.sizeof(value), None):
                raise c.WinError(c.get_last_error())
        return {"job_flags": rate.flags, "job_rate": rate.rate, "queried": True,
                "kill_on_close": bool(limits.basic.flags & 0x2000)}

    def contains(self, process, job):
        from ctypes import wintypes as w
        answer = w.BOOL()
        if not self.kernel.IsProcessInJob(process, job, self.c.byref(answer)):
            raise self.c.WinError(self.c.get_last_error())
        return bool(answer.value)

    def open_aggregate(self, name):
        require(isinstance(name, str) and name.startswith("Local\\GeoSonnarValidation-"), "Missing owned aggregate job name")
        handle = self.kernel.OpenJobObjectW(0x0004, False, name)  # JOB_OBJECT_QUERY, never inherited.
        if not handle: raise self.c.WinError(self.c.get_last_error())
        return handle


def current_job():
    api = _Windows()
    name = os.environ.get(AGGREGATE_JOB_ENV)
    handle = api.open_aggregate(name)
    try:
        proof = {"schema_version": 2, "policy": "aggregate_20_percent", "job_name": name,
                 "cpu_percent": 20, **api.query(handle)}
        verify_aggregate_proof(proof)
        require(api.contains(api.kernel.GetCurrentProcess(), handle), "Worker is not in the named aggregate job")
        return proof
    finally:
        # A persistent handle in the worker would defeat launcher kill-on-close.
        api.kernel.CloseHandle(handle)


class WindowsJob(_Windows):
    def __init__(self, aggregate=False):
        super().__init__()
        self.handle, self.is_aggregate = None, aggregate
        c = self.c
        host = None
        if aggregate:
            present = self.contains(self.kernel.GetCurrentProcess(), None)
            observed_host = self.query(None) if present else {"job_flags": 0, "job_rate": 0}
            host = {"present": present, "queried": True, "immediate_job_flags": observed_host["job_flags"],
                    "immediate_job_rate": observed_host["job_rate"], "ancestor_policies_unknown": True,
                    "external_limits_may_reduce_availability": True}
            verify_launcher_host(host)
            self.name = "Local\\GeoSonnarValidation-" + uuid.uuid4().hex
            parent = None
        else:
            self.name, parent = None, current_job()
        try:
            c.set_last_error(0)
            self.handle = self.kernel.CreateJobObjectW(None, self.name)
            if not self.handle: raise c.WinError(c.get_last_error())
            require(not aggregate or c.get_last_error() != 183, "Aggregate job name already exists")
            limits = self.Limits(); limits.basic.flags = 0x2000
            settings = [(9, limits)]
            if aggregate: settings.append((15, self.Rate(5, 2000)))
            for kind, value in settings:
                if not self.kernel.SetInformationJobObject(self.handle, kind, c.byref(value), c.sizeof(value)):
                    raise c.WinError(c.get_last_error())
            observed = self.query(self.handle)
            require((observed["job_flags"], observed["job_rate"]) == ((5, 2000) if aggregate else (0, 0)) and
                    observed["kill_on_close"], "Windows job policy was not applied")
            if aggregate:
                parent = {"schema_version": 2, "policy": "aggregate_20_percent", "job_name": self.name,
                          "cpu_percent": 20, **observed}
            self.proof = {"schema_version": 2, "policy": "aggregate_owner" if aggregate else "inherited_aggregate_no_additional_cpu_cap",
                          "aggregate": parent, "owned_job": observed,
                          "aggregate_membership_queried": False, "owned_membership_queried": False,
                          "below_normal_priority_queried": False}
            if host is not None: self.proof["launcher_host"] = host
        except BaseException:
            self.close(); raise

    def assign_and_resume(self, child):
        c, k = self.c, self.kernel
        if not k.SetPriorityClass(int(child._handle), 0x4000) or not k.AssignProcessToJobObject(self.handle, int(child._handle)):
            raise c.WinError(c.get_last_error())
        require(k.GetPriorityClass(int(child._handle)) == 0x4000, "Child priority differs")
        self.proof["below_normal_priority_queried"] = True
        require(self.contains(int(child._handle), self.handle), "Child escaped its owned job")
        self.proof["owned_membership_queried"] = True
        if self.is_aggregate:
            self.proof["aggregate_membership_queried"] = True
        else:
            handle = self.open_aggregate(self.proof["aggregate"]["job_name"])
            try:
                require(self.query(handle) == {key: self.proof["aggregate"][key] for key in
                        ("job_flags", "job_rate", "queried", "kill_on_close")}, "Aggregate policy changed")
                require(self.contains(int(child._handle), handle), "Child did not inherit aggregate membership")
                self.proof["aggregate_membership_queried"] = True
            finally:
                k.CloseHandle(handle)  # Close before resume; launcher remains the only long-lived owner.
            verify_command_proof(self.proof)
        snapshot = k.CreateToolhelp32Snapshot(0x4, 0)
        if snapshot == c.c_void_p(-1).value: raise c.WinError(c.get_last_error())
        resumed = 0
        try:
            entry = self.Thread(); entry.size = c.sizeof(entry)
            available = k.Thread32First(snapshot, c.byref(entry))
            while available:
                if entry.process_id == child.pid:
                    handle = k.OpenThread(0x2, False, entry.thread_id)
                    if not handle: raise c.WinError(c.get_last_error())
                    try:
                        if k.ResumeThread(handle) == 0xffffffff: raise c.WinError(c.get_last_error())
                        resumed += 1
                    finally: k.CloseHandle(handle)
                available = k.Thread32Next(snapshot, c.byref(entry))
        finally: k.CloseHandle(snapshot)
        require(resumed > 0, "No suspended child thread resumed")

    def close(self):
        if self.handle:
            self.kernel.CloseHandle(self.handle); self.handle = None


def capture(pipe, path, limit, stop, errors):
    try:
        retained = 0
        with Path(path).open("xb") as target:
            while chunk := pipe.read(65536):
                room = max(0, limit - retained)
                target.write(chunk[:room]); retained += min(room, len(chunk))
                if len(chunk) > room: stop.set()
    except Exception as error:
        errors.append(str(error)); stop.set()
    finally:
        pipe.close()


def run(arguments, *, cwd, log_prefix, environment=None, timeout_s=1800, log_bytes=16 * 1024 * 1024, expected=0, aggregate=False):
    require(0 < timeout_s <= 86400 and 0 < log_bytes <= 64 * 1024 * 1024, "Command limits out of range")
    prefix = Path(log_prefix); prefix.parent.mkdir(parents=True, exist_ok=True)
    stdout, stderr = prefix.with_suffix(".stdout"), prefix.with_suffix(".stderr")
    record_path = prefix.with_suffix(".command.json")
    require(not any(os.path.lexists(p) for p in (stdout, stderr, record_path)), "Fresh command logs required")
    record = {"command": list(map(str, arguments)), "cwd": str(cwd), "expected_returncode": expected,
              "environment_overrides": environment or {}, "timeout_s": timeout_s,
              "log_limit_bytes_per_stream": log_bytes, "window_opened": False, "status": "failed"}
    began, child, job, workers = time.monotonic(), None, None, []
    stop, errors = threading.Event(), []
    try:
        job = WindowsJob(aggregate=aggregate); record["job"] = job.proof
        env = os.environ.copy(); env.update(environment or {})
        if aggregate: env[AGGREGATE_JOB_ENV] = job.name
        else:
            require(env.get(AGGREGATE_JOB_ENV) == job.proof["aggregate"]["job_name"], "Aggregate binding cannot be overridden")
        child = subprocess.Popen(record["command"], cwd=cwd, env=env, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, creationflags=subprocess.CREATE_NO_WINDOW | 0x4)
        for pipe, path in ((child.stdout, stdout), (child.stderr, stderr)):
            worker = threading.Thread(target=capture, args=(pipe, path, log_bytes, stop, errors), daemon=True)
            worker.start(); workers.append(worker)
        job.assign_and_resume(child)
        while child.poll() is None:
            require(not stop.is_set(), "Command exceeded retained log bound or log capture failed")
            require(time.monotonic() - began < timeout_s, "Command wall deadline exceeded")
            time.sleep(0.05)
        record["returncode"] = child.returncode
        job.close()  # Also close any descendant that retained a pipe after its parent exited.
        for worker in workers: worker.join(timeout=10)
        require(not any(worker.is_alive() for worker in workers) and not errors and not stop.is_set(),
                "Log capture failed or exceeded its bound")
        require(child.returncode in expected if isinstance(expected, tuple) else child.returncode == expected,
                f"Expected exit {expected}, got {child.returncode}")
        record["status"] = "passed"
    except BaseException as error:
        record["error"] = type(error).__name__ + ": " + str(error)
    finally:
        if job: job.close()
        if child and child.poll() is None:
            child.kill(); child.wait(timeout=10)
        for worker in workers: worker.join(timeout=10)
        record["elapsed_s"] = time.monotonic() - began
        record["logs"] = [receipt(p) for p in (stdout, stderr) if p.is_file()]
        write(record_path, record)
    require(record["status"] == "passed", record.get("error", "Command failed") + "; see " + str(record_path))
    return record
