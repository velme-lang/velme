#!/usr/bin/env python3
"""Dev tooling: an `external` backend (compiler/22 section 3.2) that lets a coding agent stand in for the provider.

`GET /v1/describe` names the bridge and lists the reply formats it answers in. `POST /v1/synthesize` is keyed by the SHA-256 of the request body, which Velme sends
as canonical JSON, so the same request always gets the same id:

* `<answers>/<id>.json` exists: its content is the reply. For a default (`ir-json`) request a file holding an expression node
  (it has a `kind`) is wrapped as `{"body": ...}`; any other JSON object, and every answer to a request that names another
  `reply_format`, is sent as written (`{"body"}`, `{"bo"}`, `{"question"}`, `{"error"}`). A file that is not JSON yet is
  `pending`.
* otherwise the request is written to `<pending>/<id>.json` (atomically) and the reply is `{"pending": "..."}` (R-SYNTH-41),
  which Velme shows as VL0408 with the id.

Standard library only. Binds 127.0.0.1 and nothing else. With a token (`--token` or `VELME_EXTERNAL_TOKEN`) every request
must carry `Authorization: Bearer <token>`. Not part of the product; see README.md.
"""
import argparse
import hashlib
import hmac
import http.server
import json
import os
import re
import sys
import tempfile

BACKEND = "agent-bridge"
VERSION = "0.1"
# Every format answers as written, so every format Velme names is offered (22 section 3.2, D-156).
REPLY_FORMATS = ["ir-json", "compact", "notation"]
ID_PATTERN = re.compile(r"^[0-9a-f]{32}$")
MAX_REQUEST_BYTES = 8 * 1024 * 1024


def request_id(body: bytes) -> str:
    return hashlib.sha256(body).hexdigest()[:32]


def write_atomically(path: str, text: str) -> None:
    directory = os.path.dirname(path)
    handle, temporary = tempfile.mkstemp(dir=directory, prefix=".tmp-")
    try:
        with os.fdopen(handle, "w", encoding="utf-8") as out:
            out.write(text)
        os.replace(temporary, path)
    except BaseException:
        if os.path.exists(temporary):
            os.unlink(temporary)
        raise


def reply_for(answers: str, pending: str, body: bytes) -> dict:
    ident = request_id(body)
    # Defence in depth: the id is hex by construction, but a path is never built from anything else.
    if not ID_PATTERN.match(ident):
        return {"error": "bad request id"}
    try:
        request = json.loads(body)
    except ValueError:
        return {"error": "the request was not JSON"}
    answer = os.path.join(answers, ident + ".json")
    if os.path.isfile(answer):
        try:
            with open(answer, encoding="utf-8") as handle:
                doc = json.load(handle)
        except ValueError:
            # Most likely being written right now: the agent should write a temp file and rename it, but do not fail the build.
            return {"pending": "answer %s is not valid JSON yet" % ident}
        except OSError as error:
            return {"error": "answers/%s.json is not readable: %s" % (ident, error.strerror or "error")}
        if not isinstance(doc, dict):
            return {"error": "answers/%s.json is not a JSON object" % ident}
        # A bare expression node is wrapped for the default format; in any other format the answer is sent as written.
        default = not isinstance(request, dict) or request.get("reply_format", "ir-json") == "ir-json"
        return {"body": doc} if default and "kind" in doc else doc
    try:
        write_atomically(os.path.join(pending, ident + ".json"), json.dumps(request, indent=2) + "\n")
    except OSError as error:
        return {"error": "could not queue the request: %s" % (error.strerror or "error")}
    return {"pending": "queued as %s; write answers/%s.json" % (ident, ident)}


def make_handler(answers: str, pending: str, token):
    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, fmt, *args):
            sys.stderr.write("agent-bridge: %s\n" % (fmt % args))

        def send_json(self, status: int, doc: dict) -> None:
            data = json.dumps(doc).encode("utf-8")
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def authorized(self) -> bool:
            if token is None:
                return True
            given = self.headers.get("Authorization", "")
            return hmac.compare_digest(given.encode("utf-8"), ("Bearer " + token).encode("utf-8"))

        def do_GET(self):
            if not self.authorized():
                return self.send_json(401, {"error": "token required"})
            if self.path == "/v1/describe":
                return self.send_json(
                    200, {"backend": BACKEND, "backend_version": VERSION, "reply_formats": REPLY_FORMATS}
                )
            self.send_json(404, {"error": "not found"})

        def do_POST(self):
            try:
                length = int(self.headers.get("Content-Length") or 0)
            except ValueError:
                length = -1
            if not 0 <= length <= MAX_REQUEST_BYTES:
                # The body is not read, so the connection cannot be reused.
                self.close_connection = True
                return self.send_json(400, {"error": "bad Content-Length"})
            body = self.rfile.read(length)
            if not self.authorized():
                return self.send_json(401, {"error": "token required"})
            if self.path != "/v1/synthesize":
                return self.send_json(404, {"error": "not found"})
            self.send_json(200, reply_for(answers, pending, body))

    return Handler


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--answers", default="answers", help="folder of <id>.json answers (default: answers)")
    parser.add_argument("--pending", default="pending", help="folder the unanswered requests go to (default: pending)")
    parser.add_argument("--port", type=int, default=8787, help="port on 127.0.0.1; 0 picks a free one")
    parser.add_argument("--token", default=os.environ.get("VELME_EXTERNAL_TOKEN"), help="bearer token to require")
    args = parser.parse_args()
    os.makedirs(args.answers, exist_ok=True)
    os.makedirs(args.pending, exist_ok=True)
    token = args.token.strip() if args.token else None
    server = http.server.ThreadingHTTPServer(("127.0.0.1", args.port), make_handler(args.answers, args.pending, token))
    print("agent-bridge listening on http://127.0.0.1:%d" % server.server_address[1], flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
