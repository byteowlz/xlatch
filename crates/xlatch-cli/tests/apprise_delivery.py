"""Exercise daemon delivery against a local mock; no external notifications."""

import http.server, threading, tempfile, pathlib, subprocess, os, sqlite3, time, signal, json, socket
root = pathlib.Path(tempfile.mkdtemp(prefix='xlatch-apprise-'))
(root / 'config' / 'xlatch').mkdir(parents=True)
received = []

class Handler(http.server.BaseHTTPRequestHandler):

    def do_POST(self):
        received.append((self.path, json.loads(self.rfile.read(int(self.headers['Content-Length'])))))
        self.send_response(503 if len(received) == 1 else 200)
        self.end_headers()

    def log_message(self, *args):
        pass
http = http.server.HTTPServer(('127.0.0.1', 0), Handler)
threading.Thread(target=http.serve_forever, daemon=True).start()
(root / 'config' / 'xlatch' / 'notifications.toml').write_text(f'endpoint = "http://127.0.0.1:{http.server_port}/notify/test"\ntag = "phone"\n')
with socket.socket() as s:
    s.bind(('127.0.0.1', 0))
    port = s.getsockname()[1]
env = dict(os.environ, XDG_CONFIG_HOME=str(root / 'config'))
cmd = ['target/debug/xlatch', '--data-dir', str(root / 'data'), 'service', 'run', '--listen', f'127.0.0.1:{port}']
log = open(root / 'server.log', 'w')
process = None
try:
    process = subprocess.Popen(cmd, env=env, stdout=log, stderr=log)
    for _ in range(100):
        if (root / 'data' / 'control.sock').exists():
            break
        assert process.poll() is None, (root / 'server.log').read_text()
        time.sleep(0.1)
    db = sqlite3.connect(root / 'data' / 'xlatch.sqlite3')
    db.execute("INSERT INTO jobs VALUES('private-job','private-capability','rev','{}','local','succeeded','secret input','secret result',NULL,'key',0)")
    db.execute("INSERT INTO events(job_id,owner,status) VALUES('private-job','local','succeeded')")
    db.commit()
    for _ in range(100):
        if len(received) >= 2:
            break
        time.sleep(0.1)
    assert len(received) == 2, received
    expected = ('/notify/test', {'title': 'xlatch', 'body': 'A job completed. Open xlatch to view the result.', 'type': 'success', 'format': 'text', 'tag': 'phone'})
    assert received == [expected, expected], received
    time.sleep(0.3)
    process.send_signal(signal.SIGINT)
    process.wait(timeout=10)
    process = subprocess.Popen(cmd, env=env, stdout=log, stderr=log)
    time.sleep(3)
    assert process.poll() is None
    assert len(received) == 2, 'Acknowledged event replayed after restart'
    print('PASS: failed HTTP retried, generic payload only, acknowledged cursor survives daemon restart')
finally:
    if process and process.poll() is None:
        process.send_signal(signal.SIGINT)
        process.wait(timeout=10)
    http.shutdown()
    log.close()
