"""Focused stdlib checks for benchmark shutdown, failed readiness and Windows memory."""
import os
import sys
import time

_peer_launch = os.environ.get("_KTLSP_BENCH_PEER_START")
def peer_event(description):
    if _peer_launch and sys.argv[1:] == ["--protocol-peer"]:
        elapsed = (time.monotonic() - float(_peer_launch)) * 1000
        print(f"protocol peer {description} after {elapsed:.1f} ms", file=sys.stderr, flush=True)

peer_event("Python entry")

import io
import json
import queue
import subprocess
import unittest
from pathlib import Path
from unittest.mock import patch

import bench_initialize as bench
peer_event("imports ready")


class Messages(queue.Queue):
    def __init__(self, events):
        super().__init__()
        self.events = events

    def get(self, *args, **kwargs):
        item = super().get(*args, **kwargs)
        if item[1].get("id") == 99:
            self.events.append("shutdown response read")
        return item


class Process:
    """A transport peer that rejects exit before the client reads shutdown's response."""
    def __init__(self, shutdown_reply=True, exit_hangs=False, initialize=True, project=False):
        self.events = []
        self.messages = Messages(self.events)
        self.shutdown_reply, self.exit_hangs = shutdown_reply, exit_hangs
        self.initialize, self.project = initialize, project
        self.returncode, self.pid = None, os.getpid()
        self.stdin = io.BytesIO()
        self.stderr = io.BytesIO()
        self.stdout = io.BytesIO()

    def send(self, proc, payload):
        method = payload["method"]
        self.events.append(method)
        if method == "initialize" and self.initialize:
            self.messages.put((bench.time.monotonic(), {"id": 1, "result": {"capabilities": {}}}))
        elif method == "initialized" and self.project:
            self.messages.put((bench.time.monotonic(), {"method": "window/logMessage",
                "params": {"message": "ktlsp indexed 2 project files"}}))
        elif method == "shutdown" and self.shutdown_reply:
            self.messages.put((bench.time.monotonic(), {"id": 99, "result": None}))
        elif method == "exit":
            if "shutdown response read" not in self.events:
                raise AssertionError("exit arrived before the shutdown response was consumed")

    def poll(self):
        return self.returncode

    def wait(self, timeout=None):
        self.events.append("wait" if timeout is None else "bounded wait")
        if self.returncode is None and self.exit_hangs:
            raise subprocess.TimeoutExpired("fake-server", timeout)
        if self.returncode is None:
            self.returncode = 0
        return self.returncode

    def kill(self):
        self.events.append("kill")
        self.returncode = -9


class BenchmarkChecks(unittest.TestCase):
    def stop(self, proc):
        with patch.object(bench, "send_message", proc.send):
            return bench.stop_process(proc, proc.messages, [], timeout_s=0.001)

    def test_shutdown_waits_for_response_before_exit(self):
        proc = Process()
        result = self.stop(proc)
        self.assertTrue(result["graceful"])
        self.assertEqual(proc.events, ["shutdown", "shutdown response read", "exit", "bounded wait", "wait"])

    def test_missing_shutdown_response_kills_and_reaps(self):
        proc = Process(shutdown_reply=False)
        result = self.stop(proc)
        self.assertFalse(result["graceful"])
        self.assertTrue(result["forced_termination"])
        self.assertFalse(result["exit_sent"])
        self.assertEqual(proc.events[-2:], ["kill", "wait"])

    def test_exit_timeout_kills_and_reaps(self):
        proc = Process(exit_hangs=True)
        result = self.stop(proc)
        self.assertTrue(result["shutdown_ok"])
        self.assertFalse(result["graceful"])
        self.assertTrue(result["forced_termination"])
        self.assertEqual(proc.events[-2:], ["kill", "wait"])

    def test_shutdown_allows_bounded_large_index_cleanup(self):
        class DelayedExit(Process):
            def wait(self, timeout=None):
                if self.returncode is None and timeout is not None and timeout < 5:
                    raise subprocess.TimeoutExpired("large-index-cleanup", timeout)
                return super().wait(timeout)

        proc = DelayedExit()
        with patch.object(bench, "send_message", proc.send):
            result = bench.stop_process(proc, proc.messages, [])
        self.assertTrue(result["graceful"])
        self.assertFalse(result["forced_termination"])
        self.assertGreaterEqual(result["elapsed_ms"], 0)

    def run_benchmark(self, proc):
        with patch.object(bench.subprocess, "Popen", return_value=proc), \
             patch.object(bench, "JsonRpcReader"), \
             patch.object(bench.queue, "Queue", return_value=proc.messages), \
             patch.object(bench, "send_message", proc.send), \
             patch.object(bench, "sample_memory", return_value={"available": False, "reason": "test"}):
            return bench.benchmark(Path("server"), Path.cwd(), None, 0.001, False, False,
                                   False, "openFilesOnly", memory_window_s=0)

    def test_missing_project_readiness_is_failure_despite_clean_shutdown(self):
        result = self.run_benchmark(Process())
        self.assertFalse(result["success"])
        self.assertEqual(result["missing_readiness"], ["project_index"])
        self.assertTrue(result["shutdown"]["graceful"])

    def test_initialize_timeout_is_json_failure_and_reaps_process(self):
        proc = Process(initialize=False)
        result = self.run_benchmark(proc)
        self.assertFalse(result["success"])
        self.assertEqual(result["missing_readiness"], ["initialize"])
        self.assertEqual(result["error"], "initialize response timeout")
        self.assertEqual(proc.events[-1], "wait")

    def test_ready_project_and_clean_shutdown_succeed(self):
        result = self.run_benchmark(Process(project=True))
        self.assertTrue(result["success"])
        self.assertTrue(result["readiness_complete"])
        self.assertEqual(result["time_origin"], "immediately before process spawn")

    def test_memory_reports_native_values_or_explicit_unavailability(self):
        memory = bench.sample_memory(os.getpid(), True, 0)
        if os.name == "nt":
            self.assertTrue(memory["available"], memory)
            self.assertGreater(memory["steady_working_set_bytes"], 0)
            self.assertGreaterEqual(memory["peak_working_set_bytes"], memory["steady_working_set_bytes"])
            incomplete = bench.sample_memory(os.getpid(), False, 0)
            self.assertIsNone(incomplete["steady_working_set_bytes"])
        else:
            self.assertFalse(memory["available"])
            self.assertTrue(memory["reason"])

    def test_live_protocol_child_reports_ready_memory_and_clean_exit(self):
        popen = subprocess.Popen
        peer = Path(__file__).resolve()
        def launch(*args, **kwargs):
            kwargs["env"] = {**os.environ, "_KTLSP_BENCH_PEER_START": str(time.monotonic())}
            return popen([sys.executable, "-B", str(peer), "--protocol-peer"], **kwargs)
        with patch.object(bench.subprocess, "Popen", side_effect=launch):
            result = bench.benchmark(Path("peer"), Path.cwd(), None, 2, True, False,
                                     False, "openFilesOnly", memory_window_s=0.05)
        self.assertTrue(result["success"], result)
        self.assertTrue(result["shutdown"]["shutdown_response_received"])
        self.assertEqual(result["shutdown"]["exit_code"], 0)
        self.assertIn("protocol peer received exit", result["stderr_tail"])
        if os.environ.get("KTLSP_BENCH_TEST_TRACE"):
            print("\n".join(result["stderr_tail"]))
        if os.name == "nt":
            self.assertTrue(result["memory"]["available"], result["memory"])
            self.assertGreater(result["memory"]["steady_working_set_bytes"], 0)


def protocol_peer():
    def reply(payload):
        body = json.dumps(payload).encode()
        sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
        sys.stdout.buffer.flush()

    while True:
        headers = {}
        while True:
            line = sys.stdin.buffer.readline()
            if not line:
                return
            if line == b"\r\n":
                break
            key, value = line.decode().split(":", 1)
            headers[key.lower()] = value.strip()
        message = json.loads(sys.stdin.buffer.read(int(headers["content-length"])))
        method = message.get("method")
        peer_event("read " + str(method))
        if method == "initialize":
            reply({"jsonrpc": "2.0", "id": message["id"], "result": {"capabilities": {}}})
        elif method == "initialized":
            for description in ("2 project files", "1 library files"):
                reply({"jsonrpc": "2.0", "method": "window/logMessage",
                       "params": {"message": "ktlsp indexed " + description}})
        elif method == "shutdown":
            reply({"jsonrpc": "2.0", "id": message["id"], "result": None})
        elif method == "exit":
            print("protocol peer received exit", file=sys.stderr)
            return


if __name__ == "__main__":
    if sys.argv[1:] == ["--protocol-peer"]:
        protocol_peer()
    else:
        unittest.main()
