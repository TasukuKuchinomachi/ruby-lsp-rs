import json
import queue
import subprocess
import threading
import time


class LanguageServerError(Exception):
    pass


class LanguageServerClient:
    def __init__(self, command, working_directory, log_path=None, server_request_handler=None):
        self.log_file = open(log_path, "a") if log_path else None
        self.stderr_file = open(log_path + ".stderr", "a") if log_path else subprocess.DEVNULL
        self.process = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self.stderr_file,
            cwd=working_directory,
        )
        self.incoming_messages = queue.Queue()
        self.notifications = []
        self.notification_condition = threading.Condition()
        self.pending_responses = {}
        self.response_condition = threading.Condition()
        self.server_request_handler = server_request_handler
        self.write_lock = threading.Lock()
        self.next_request_id = 1
        self.reader_thread = threading.Thread(target=self._read_loop, daemon=True)
        self.reader_thread.start()

    def _log(self, direction, message):
        if self.log_file:
            self.log_file.write(f"{time.time():.3f} {direction} {json.dumps(message, ensure_ascii=False)[:2000]}\n")
            self.log_file.flush()

    def _read_loop(self):
        stream = self.process.stdout
        while True:
            headers = {}
            while True:
                line = stream.readline()
                if not line:
                    with self.response_condition:
                        self.response_condition.notify_all()
                    return
                line = line.decode("ascii", errors="replace").strip()
                if not line:
                    break
                key, value = line.split(":", 1)
                headers[key.strip().lower()] = value.strip()
            body = stream.read(int(headers["content-length"]))
            message = json.loads(body)
            self._log("<-", message)
            self._dispatch(message)

    def _dispatch(self, message):
        if "method" in message and "id" in message:
            result = None
            if self.server_request_handler:
                result = self.server_request_handler(self, message)
            self._write({"jsonrpc": "2.0", "id": message["id"], "result": result})
            with self.notification_condition:
                self.notifications.append(message)
                self.notification_condition.notify_all()
        elif "method" in message:
            with self.notification_condition:
                self.notifications.append(message)
                self.notification_condition.notify_all()
        else:
            with self.response_condition:
                self.pending_responses[message.get("id")] = message
                self.response_condition.notify_all()

    def _write(self, message):
        self._log("->", message)
        body = json.dumps(message).encode()
        with self.write_lock:
            self.process.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
            self.process.stdin.flush()

    def notify(self, method, params):
        self._write({"jsonrpc": "2.0", "method": method, "params": params})

    def request(self, method, params, timeout=120):
        request_id = self.next_request_id
        self.next_request_id += 1
        self._write({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        deadline = time.time() + timeout
        with self.response_condition:
            while request_id not in self.pending_responses:
                remaining = deadline - time.time()
                if remaining <= 0:
                    raise LanguageServerError(f"{method} timed out after {timeout}s")
                if self.process.poll() is not None:
                    raise LanguageServerError(f"server exited while waiting for {method}")
                self.response_condition.wait(min(remaining, 1.0))
            response = self.pending_responses.pop(request_id)
        if "error" in response:
            raise LanguageServerError(f"{method}: {response['error']}")
        return response.get("result")

    def wait_for_notification(self, predicate, timeout, start_index=0):
        deadline = time.time() + timeout
        with self.notification_condition:
            while True:
                for message in self.notifications[start_index:]:
                    if predicate(message):
                        return message
                start_index = len(self.notifications)
                remaining = deadline - time.time()
                if remaining <= 0:
                    return None
                self.notification_condition.wait(min(remaining, 1.0))

    def notification_count(self):
        with self.notification_condition:
            return len(self.notifications)

    def shutdown(self):
        try:
            self.request("shutdown", None, timeout=30)
            self.notify("exit", None)
            self.process.wait(timeout=30)
        except (LanguageServerError, subprocess.TimeoutExpired, BrokenPipeError, OSError):
            self.process.kill()
        if self.log_file:
            self.log_file.close()
