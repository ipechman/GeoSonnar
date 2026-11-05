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
from tools.evidence import receipt, require, write


class WindowsJob:
    def __init__(self):
        require(os.name == "nt", "Capped execution currently requires Windows; no uncapped fallback")
        import ctypes as c
        from ctypes import wintypes as w
        self.c, self.kernel, self.handle = c, c.WinDLL("kernel32", use_last_error=True), None
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
        self.Thread = Thread
        signatures = {
            "CreateJobObjectW": ([c.c_void_p, w.LPCWSTR], w.HANDLE),
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
        try:
            self.handle = self.kernel.CreateJobObjectW(None, None)
            if not self.handle: raise c.WinError(c.get_last_error())
            limits = Limits(); limits.basic.flags = 0x2000
            rate = Rate(5, 2000)
            for kind, value in ((9, limits), (15, rate)):
                if not self.kernel.SetInformationJobObject(self.handle, kind, c.byref(value), c.sizeof(value)):
                    raise c.WinError(c.get_last_error())
            observed = Rate()
            if not self.kernel.QueryInformationJobObject(self.handle, 15, c.byref(observed), c.sizeof(observed), None):
                raise c.WinError(c.get_last_error())
            require((observed.flags, observed.rate) == (5, 2000), "Windows CPU cap was not applied")
            self.proof = {"cpu_percent": 20, "job_flags": observed.flags, "job_rate": observed.rate,
                          "queried": True, "kill_on_close": True, "below_normal_priority": True}
        except BaseException:
            self.close(); raise

    def assign_and_resume(self, child):
        c, k = self.c, self.kernel
        if not k.SetPriorityClass(int(child._handle), 0x4000) or not k.AssignProcessToJobObject(self.handle, int(child._handle)):
            raise c.WinError(c.get_last_error())
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


def current_job():
    require(os.name == "nt", "Capped execution currently requires Windows")
    import ctypes as c
    from ctypes import wintypes as w
    class Rate(c.Structure):
        _fields_ = [("flags", w.DWORD), ("rate", w.DWORD)]
    kernel = c.WinDLL("kernel32", use_last_error=True)
    query = kernel.QueryInformationJobObject
    query.argtypes = [w.HANDLE, c.c_int, c.c_void_p, w.DWORD, c.c_void_p]; query.restype = w.BOOL
    rate = Rate()
    require(bool(query(None, 15, c.byref(rate), c.sizeof(rate), None)) and (rate.flags, rate.rate) == (5, 2000),
            "Worker is not inside the required 20% CPU job")
    return {"cpu_percent": 20, "job_flags": rate.flags, "job_rate": rate.rate, "queried": True}


def run(arguments, *, cwd, log_prefix, environment=None, timeout_s=1800, log_bytes=16 * 1024 * 1024, expected=0):
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
        job = WindowsJob(); record["job"] = job.proof
        env = os.environ.copy(); env.update(environment or {})
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
