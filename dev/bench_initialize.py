#!/usr/bin/env python3

import argparse
import json
import os
import queue
import statistics
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple


def uri_for(path: Path) -> str:
    return path.resolve().as_uri()


class JsonRpcReader(threading.Thread):
    def __init__(self, stream, out_queue: "queue.Queue[Tuple[float, Dict[str, Any]]]"):
        super().__init__(daemon=True)
        self.stream = stream
        self.out_queue = out_queue

    def run(self) -> None:
        try:
            while True:
                headers = {}
                while True:
                    line = self.stream.readline()
                    if not line:
                        return
                    if line == b"\r\n":
                        break
                    key, value = line.decode("utf-8").split(":", 1)
                    headers[key.strip().lower()] = value.strip()
                length = int(headers.get("content-length", "0"))
                if length <= 0:
                    continue
                payload = self.stream.read(length)
                if not payload:
                    return
                message = json.loads(payload.decode("utf-8"))
                self.out_queue.put((time.monotonic(), message))
        except Exception as exc:
            self.out_queue.put((time.monotonic(), {"reader_error": str(exc)}))


def send_message(proc: subprocess.Popen[bytes], payload: dict[str, Any]) -> None:
    body = json.dumps(payload).encode("utf-8")
    header = f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8")
    assert proc.stdin is not None
    proc.stdin.write(header)
    proc.stdin.write(body)
    proc.stdin.flush()


def wait_for_message(
    messages: "queue.Queue[Tuple[float, Dict[str, Any]]]",
    predicate,
    timeout_s: float,
    stash: List[Tuple[float, Dict[str, Any]]],
):
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        remaining = max(0.0, deadline - time.monotonic())
        try:
            item = messages.get(timeout=remaining)
        except queue.Empty:
            break
        ts, msg = item
        stash.append(item)
        if predicate(msg):
            return ts, msg
    return None, None


def stop_process(proc, messages, stash, timeout_s: float = 15.0) -> Dict[str, Any]:
    started = time.monotonic()
    status = {"shutdown_response_received": False, "shutdown_ok": False,
              "exit_sent": False, "forced_termination": False}
    try:
        send_message(proc, {"jsonrpc": "2.0", "id": 99, "method": "shutdown", "params": None})
        _, response = wait_for_message(messages, lambda msg: msg.get("id") == 99, timeout_s, stash)
        status["shutdown_response_received"] = response is not None
        status["shutdown_ok"] = bool(response is not None and "result" in response
                                     and response["result"] is None and "error" not in response)
        if response is None:
            status["error"] = "shutdown response timeout"
        else:
            if "error" in response:
                status["error"] = response["error"]
            send_message(proc, {"jsonrpc": "2.0", "method": "exit", "params": None})
            status["exit_sent"] = True
            proc.wait(timeout=timeout_s)
    except (OSError, subprocess.TimeoutExpired) as exc:
        status["error"] = str(exc)
    finally:
        if proc.poll() is None:
            status["forced_termination"] = True
            proc.kill()
        proc.wait()  # Reap after both graceful exit and forced termination.
    status["exit_code"] = proc.returncode
    status["elapsed_ms"] = round((time.monotonic() - started) * 1000, 1)
    status["graceful"] = (status["shutdown_ok"] and status["exit_sent"]
                          and not status["forced_termination"] and proc.returncode == 0)
    return status


def process_memory(pid: int) -> Dict[str, Any]:
    if os.name != "nt":
        return {"available": False, "reason": "working-set measurement is implemented for Windows only"}
    import ctypes
    from ctypes import wintypes

    class Counters(ctypes.Structure):
        _fields_ = [("cb", wintypes.DWORD), ("PageFaultCount", wintypes.DWORD)] + [
            (name, ctypes.c_size_t) for name in (
                "PeakWorkingSetSize", "WorkingSetSize", "QuotaPeakPagedPoolUsage",
                "QuotaPagedPoolUsage", "QuotaPeakNonPagedPoolUsage", "QuotaNonPagedPoolUsage",
                "PagefileUsage", "PeakPagefileUsage")]

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.OpenProcess.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = (wintypes.HANDLE,)
    kernel.CloseHandle.restype = wintypes.BOOL
    memory = ctypes.WinDLL("psapi", use_last_error=True).GetProcessMemoryInfo
    memory.argtypes = (wintypes.HANDLE, ctypes.POINTER(Counters), wintypes.DWORD)
    memory.restype = wintypes.BOOL
    handle = kernel.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not handle:
        return {"available": False, "reason": str(ctypes.WinError(ctypes.get_last_error()))}
    try:
        counters = Counters()
        counters.cb = ctypes.sizeof(counters)
        if not memory(handle, ctypes.byref(counters), counters.cb):
            return {"available": False, "reason": str(ctypes.WinError(ctypes.get_last_error()))}
        return {"available": True, "working_set_bytes": counters.WorkingSetSize,
                "peak_working_set_bytes": counters.PeakWorkingSetSize}
    finally:
        kernel.CloseHandle(handle)


def sample_memory(pid: int, ready: bool, window_s: float) -> Dict[str, Any]:
    sample = process_memory(pid)
    if not sample["available"]:
        return sample
    start = time.monotonic()
    samples = [sample["working_set_bytes"]]
    peak = sample["peak_working_set_bytes"]
    deadline = start + (window_s if ready else 0)
    while time.monotonic() < deadline:
        time.sleep(min(0.1, max(0, deadline - time.monotonic())))
        sample = process_memory(pid)
        if not sample["available"]:
            break
        samples.append(sample["working_set_bytes"])
        peak = max(peak, sample["peak_working_set_bytes"])
    result = {"available": True, "source": "GetProcessMemoryInfo",
              "phase": "post-readiness" if ready else "incomplete-readiness",
              "sample_window_ms": round((time.monotonic() - start) * 1000, 1),
              "working_set_samples_bytes": samples,
              "steady_working_set_bytes": int(statistics.median(samples)) if ready else None,
              "peak_working_set_bytes": peak,
              "peak_scope": "process lifetime through final pre-shutdown sample"}
    if not sample["available"]:
        result["sampling_error"] = sample["reason"]
    return result


def benchmark(
    binary: Path,
    root: Path,
    probe_file: Optional[Path],
    timeout_s: float,
    wait_for_libraries: bool,
    request_document_diagnostic: bool,
    request_workspace_diagnostic: bool,
    diagnostics_scope: str,
    memory_window_s: float = 1.0,
) -> Dict[str, Any]:
    probe_text = probe_file.read_text(encoding="utf-8") if probe_file is not None else None
    start = time.monotonic()
    proc = subprocess.Popen(
        [str(binary)],
        cwd=str(root),
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    spawn_ms = round((time.monotonic() - start) * 1000, 1)
    assert proc.stdout is not None
    assert proc.stderr is not None
    messages: "queue.Queue[Tuple[float, Dict[str, Any]]]" = queue.Queue()
    reader = JsonRpcReader(proc.stdout, messages)
    reader.start()
    stderr_lines: List[str] = []

    def read_stderr():
        for line in proc.stderr:
            stderr_lines.append(line.decode("utf-8", errors="replace").rstrip())

    stderr_thread = threading.Thread(target=read_stderr, daemon=True)
    stderr_thread.start()

    init_id = 1
    send_message(
        proc,
        {
            "jsonrpc": "2.0",
            "id": init_id,
            "method": "initialize",
            "params": {
                "processId": os.getpid(),
                "rootUri": uri_for(root),
                "capabilities": {
                    "window": {"workDoneProgress": True},
                },
                "initializationOptions": {
                    "diagnostics": {"scope": diagnostics_scope},
                    "compile_diagnostics": {"enabled": False},
                },
                "workspaceFolders": [{"uri": uri_for(root), "name": root.name}],
            },
        },
    )

    stash: List[Tuple[float, Dict[str, Any]]] = []
    init_ts, init_msg = wait_for_message(
        messages,
        lambda msg: msg.get("id") == init_id,
        timeout_s,
        stash,
    )
    if init_msg is None or "error" in init_msg or not isinstance(init_msg.get("result"), dict):
        memory = sample_memory(proc.pid, False, memory_window_s)
        shutdown = stop_process(proc, messages, stash)
        reader.join(timeout=1)
        stderr_thread.join(timeout=1)
        for stream in (proc.stdin, proc.stdout, proc.stderr):
            if stream is not None:
                stream.close()
        return {"binary": str(binary), "root": str(root), "spawn_ms": spawn_ms,
                "time_origin": "immediately before process spawn",
                "initialize_ms": round((init_ts - start) * 1000, 1) if init_ts else None,
                "initialize_response": init_msg,
                "response_events": [{"t_ms": round((ts - start) * 1000, 1), "response": message}
                                    for ts, message in stash if message.get("id") in (init_id, 99)],
                "error": "initialize response timeout" if init_msg is None else "initialize failed",
                "readiness_complete": False, "missing_readiness": ["initialize"],
                "success": False, "shutdown": shutdown, "memory": memory,
                "stderr_tail": stderr_lines[-20:]}

    send_message(proc, {"jsonrpc": "2.0", "method": "initialized", "params": {}})

    did_open_sent_at = None
    semantic_tokens_id = 2
    document_diagnostic_id = 3
    workspace_diagnostic_id = 4
    semantic_response_ts = None
    semantic_response = None
    document_diagnostic_response_ts = None
    document_diagnostic_response = None
    workspace_diagnostic_response_ts = None
    workspace_diagnostic_response = None
    if probe_file is not None:
        send_message(
            proc,
            {
                "jsonrpc": "2.0",
                "method": "textDocument/didOpen",
                "params": {
                    "textDocument": {
                        "uri": uri_for(probe_file),
                        "languageId": "java" if probe_file.suffix == ".java" else "kotlin",
                        "version": 1,
                        "text": probe_text,
                    }
                },
            },
        )
        did_open_sent_at = time.monotonic()
        send_message(
            proc,
            {
                "jsonrpc": "2.0",
                "id": semantic_tokens_id,
                "method": "textDocument/semanticTokens/full",
                "params": {"textDocument": {"uri": uri_for(probe_file)}},
            },
        )
        if request_document_diagnostic:
            send_message(
                proc,
                {
                    "jsonrpc": "2.0",
                    "id": document_diagnostic_id,
                    "method": "textDocument/diagnostic",
                    "params": {
                        "textDocument": {"uri": uri_for(probe_file)},
                    },
                },
            )
    if request_workspace_diagnostic:
        send_message(
            proc,
            {
                "jsonrpc": "2.0",
                "id": workspace_diagnostic_id,
                "method": "workspace/diagnostic",
                "params": {
                    "previousResultIds": [],
                },
            },
        )

    project_index_ts = None
    library_index_ts = None
    progress_events = []
    log_messages = []

    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        remaining = max(0.0, deadline - time.monotonic())
        try:
            ts, msg = messages.get(timeout=remaining)
        except queue.Empty:
            break
        stash.append((ts, msg))
        if msg.get("method") == "window/workDoneProgress/create" and "id" in msg:
            send_message(
                proc,
                {
                    "jsonrpc": "2.0",
                    "id": msg["id"],
                    "result": None,
                },
            )
            continue
        if msg.get("id") == semantic_tokens_id:
            semantic_response_ts = ts
            semantic_response = msg
        elif msg.get("id") == document_diagnostic_id:
            document_diagnostic_response_ts = ts
            document_diagnostic_response = msg
        elif msg.get("id") == workspace_diagnostic_id:
            workspace_diagnostic_response_ts = ts
            workspace_diagnostic_response = msg
        method = msg.get("method")
        if method == "window/logMessage":
            message = msg.get("params", {}).get("message", "")
            log_messages.append({"t_ms": round((ts - start) * 1000, 1), "message": message})
            if message.startswith("ktlsp indexed ") and "project files" in message and project_index_ts is None:
                project_index_ts = ts
            elif message.startswith("ktlsp indexed ") and "library files" in message and library_index_ts is None:
                library_index_ts = ts
        elif method == "$/progress":
            progress_events.append({"t_ms": round((ts - start) * 1000, 1), "params": msg.get("params")})
        indexing_complete = project_index_ts is not None and (
            library_index_ts is not None or not wait_for_libraries
        )
        if indexing_complete and (
            probe_file is None or semantic_response_ts is not None
        ) and (
            not request_document_diagnostic or document_diagnostic_response_ts is not None
        ) and (
            not request_workspace_diagnostic or workspace_diagnostic_response_ts is not None
        ):
            break

    readiness = {"project_index": project_index_ts is not None}
    if wait_for_libraries:
        readiness["library_index"] = library_index_ts is not None
    responses = {}
    if probe_file is not None:
        responses["semantic_tokens"] = semantic_response
    if request_document_diagnostic:
        responses["document_diagnostic"] = document_diagnostic_response
    if request_workspace_diagnostic:
        responses["workspace_diagnostic"] = workspace_diagnostic_response
    response_errors = {}
    for name, response in responses.items():
        readiness[name] = response is not None and response.get("result") is not None and "error" not in response
        if response is not None and "error" in response:
            response_errors[name] = response["error"]
    missing = [name for name, complete in readiness.items() if not complete]
    memory = sample_memory(proc.pid, not missing, memory_window_s)
    shutdown = stop_process(proc, messages, stash)
    reader.join(timeout=1)
    stderr_thread.join(timeout=1)
    for stream in (proc.stdin, proc.stdout, proc.stderr):
        if stream is not None:
            stream.close()

    result: Dict[str, Any] = {
        "binary": str(binary),
        "root": str(root),
        "time_origin": "immediately before process spawn",
        "spawn_ms": spawn_ms,
        "readiness": readiness,
        "readiness_complete": not missing,
        "missing_readiness": missing,
        "response_errors": response_errors,
        "shutdown": shutdown,
        "memory": memory,
        "success": not missing and shutdown["graceful"],
        "initialize_ms": round((init_ts - start) * 1000, 1) if init_ts is not None else None,
        "project_index_ms": round((project_index_ts - start) * 1000, 1) if project_index_ts else None,
        "library_index_ms": round((library_index_ts - start) * 1000, 1) if library_index_ts else None,
        "progress_events": progress_events,
        "log_messages": log_messages,
        "stderr_tail": stderr_lines[-20:],
        "initialize_result_keys": sorted(init_msg.get("result", {}).keys()) if init_msg else None,
        "initialize_response": init_msg,
        "methods_seen": sorted(
            {
                msg["method"]
                for _, msg in stash
                if isinstance(msg, dict) and "method" in msg
            }
        ),
        "method_counts": {
            method: sum(1 for _, msg in stash if isinstance(msg, dict) and msg.get("method") == method)
            for method in sorted(
                {
                    msg["method"]
                    for _, msg in stash
                    if isinstance(msg, dict) and "method" in msg
                }
            )
        },
    }
    if probe_file is not None:
        result["probe_file"] = str(probe_file)
        result["semantic_tokens_ms"] = (
            round((semantic_response_ts - did_open_sent_at) * 1000, 1)
            if semantic_response_ts is not None and did_open_sent_at is not None
            else None
        )
        if semantic_response is not None:
            data = (semantic_response.get("result") or {}).get("data")
            result["semantic_tokens_count"] = len(data) // 5 if isinstance(data, list) else None
            result["semantic_ok"] = "result" in semantic_response and "error" not in semantic_response
    if request_document_diagnostic:
        result["document_diagnostic_ms"] = (
            round((document_diagnostic_response_ts - did_open_sent_at) * 1000, 1)
            if document_diagnostic_response_ts is not None and did_open_sent_at is not None
            else None
        )
        if document_diagnostic_response is not None:
            items = (
                (document_diagnostic_response.get("result") or {})
                .get("fullDocumentDiagnosticReport", {})
                .get("items")
            )
            if items is None:
                items = (
                    (document_diagnostic_response.get("result") or {})
                    .get("Full", {})
                    .get("fullDocumentDiagnosticReport", {})
                    .get("items")
                )
            result["document_diagnostic_ok"] = (
                "result" in document_diagnostic_response and "error" not in document_diagnostic_response
            )
            result["document_diagnostic_items"] = len(items) if isinstance(items, list) else None
            if "error" in document_diagnostic_response:
                result["document_diagnostic_error"] = document_diagnostic_response["error"]
    if request_workspace_diagnostic:
        result["workspace_diagnostic_ms"] = (
            round((workspace_diagnostic_response_ts - start) * 1000, 1)
            if workspace_diagnostic_response_ts is not None
            else None
        )
        if workspace_diagnostic_response is not None:
            items = (workspace_diagnostic_response.get("result") or {}).get("items")
            result["workspace_diagnostic_ok"] = (
                "result" in workspace_diagnostic_response and "error" not in workspace_diagnostic_response
            )
            result["workspace_diagnostic_items"] = len(items) if isinstance(items, list) else None
            if "error" in workspace_diagnostic_response:
                result["workspace_diagnostic_error"] = workspace_diagnostic_response["error"]
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description="Benchmark ktlsp initialize/indexing against a workspace root")
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--probe-file", type=Path)
    parser.add_argument("--timeout", type=float, default=120.0)
    parser.add_argument("--steady-memory-seconds", type=float, default=1.0,
                        help="Windows working-set sampling window after readiness (default: 1 second)")
    parser.add_argument(
        "--project-only",
        action="store_true",
        help="stop after project indexing instead of waiting for library/JDK indexing",
    )
    parser.add_argument("--document-diagnostic", action="store_true")
    parser.add_argument("--workspace-diagnostic", action="store_true")
    parser.add_argument(
        "--diagnostics-scope",
        choices=["openFilesOnly", "workspace"],
        default="openFilesOnly",
    )
    args = parser.parse_args()
    if args.timeout <= 0 or args.steady_memory_seconds < 0:
        parser.error("--timeout must be positive and --steady-memory-seconds nonnegative")

    result = benchmark(
        args.binary,
        args.root,
        args.probe_file,
        args.timeout,
        not args.project_only,
        args.document_diagnostic,
        args.workspace_diagnostic,
        args.diagnostics_scope,
        args.steady_memory_seconds,
    )
    json.dump(result, sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0 if result["success"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
