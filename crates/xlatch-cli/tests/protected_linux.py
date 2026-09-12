"""OS boundary test. Run only in a disposable Linux container as root.
Requires python3-cryptography, systemd tools, and the built target/debug/xlatch.
"""
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import ssl
import subprocess
import time
import urllib.request

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import ed25519, ec

assert os.geteuid() == 0 and Path('/.dockerenv').exists()
assert os.environ.get('XLATCH_PROTECTED_TEST') == '1', 'Requires explicit disposable-container opt-in'
# The marker permits repeating this test only in its own disposable container.
marker = Path('/tmp/xlatch-boundary-owned')
if marker.exists():
    for user in ['_xlatch','agent']:
        if subprocess.run(['id',user],capture_output=True).returncode == 0:
            subprocess.run(['userdel',user],check=True)
        if subprocess.run(['getent','group',user],capture_output=True).returncode == 0:
            subprocess.run(['groupdel',user],check=True)
    for directory in ['/home/agent','/var/lib/xlatch']:
        shutil.rmtree(directory,ignore_errors=True)
    for label in ['protected','executor']:
        Path(f'/etc/systemd/system/com.byteowlz.xlatch.{label}.service').unlink(missing_ok=True)
marker.touch()
subprocess.run(['useradd', '-m', '-u', '1001', 'agent'], check=True)
BINARY = '/usr/local/bin/xlatch'
shutil.copy2('target/debug/xlatch', BINARY)
SOURCE = Path('/home/agent/xlatch')
ROOT = Path('/var/lib/xlatch')
PORT = '127.0.0.1:17898'
processes = []


def run(args, user=None, success=True):
    command = ['runuser', '-u', user, '--', *args] if user else args
    result = subprocess.run(command, text=True, capture_output=True)
    if success:
        assert result.returncode == 0, result.stderr
    return result


def cli(*args, user='agent', control=False, success=True):
    prefix = ['--control-dir', str(ROOT / 'run')] if control else ['--data-dir', str(SOURCE)]
    return run([BINARY, *prefix, *args], user=user, success=success)


def spawn(user, *args):
    log = open(f'/tmp/xlatch-{len(processes)}.log', 'w')
    child = subprocess.Popen(['runuser', '-u', user, '--', *args], stdout=log, stderr=log)
    processes.append(child)
    return child


def wait_for(path):
    for _ in range(100):
        if path.exists():
            return
        time.sleep(.1)
    raise AssertionError(f'{path} did not appear')


try:
    bootstrap = spawn('agent', BINARY, '--data-dir', str(SOURCE), 'service', 'run', '--listen', PORT)
    wait_for(SOURCE / 'control.sock')
    echo = json.loads(cli('register', 'examples/capabilities/echo.json').stdout)
    cli('approve', 'echo', '--revision', echo['revision'])
    program = str(Path('/usr/bin/python3').resolve())
    probe = "import os,json; blocked=[]\nfor p in ['/var/lib/xlatch/data/xlatch.sqlite3','/var/lib/xlatch/xlatch','/var/lib/xlatch/protected.toml']:\n try: open(p,'ab').close()\n except PermissionError: blocked.append(p)\nprint(json.dumps({'uid':os.getuid(),'blocked':blocked}))"
    manifest = dict(echo['manifest'])
    manifest.update(id='identity', title='Identity', output_schema={'type':'object'}, execution={'kind':'command', 'program':program, 'args':['-c',probe], 'sha256':hashlib.sha256(Path(program).read_bytes()).hexdigest()})
    Path('/tmp/identity.json').write_text(json.dumps(manifest))
    identity = json.loads(cli('register','/tmp/identity.json').stdout)
    cli('approve','identity','--revision',identity['revision'],'--allow-host-execution')
    bootstrap.terminate(); bootstrap.wait(timeout=10)
    key = ed25519.Ed25519PrivateKey.generate()
    public = base64.b64encode(key.public_key().public_bytes(serialization.Encoding.Raw,serialization.PublicFormat.Raw)).decode()
    approval = ec.generate_private_key(ec.SECP256R1())
    approval_public = base64.b64encode(approval.public_key().public_bytes(serialization.Encoding.X962,serialization.PublicFormat.UncompressedPoint)).decode()
    with sqlite3.connect(SOURCE/'xlatch.sqlite3') as conn:
        server_id = conn.execute('SELECT server_id FROM enrollment_policy').fetchone()[0]
        conn.execute("INSERT INTO devices(id,name,public_key) VALUES('phone','Test phone',?)",[public])
        conn.execute("INSERT INTO approvers VALUES('phone',?)",[approval_public])
        conn.execute('UPDATE enrollment_policy SET enabled=1')
        for cap in [echo,identity]:
            conn.execute("INSERT INTO grants VALUES('phone',?,?)",[cap['manifest']['id'],cap['revision']])
    frame = f'xlatch.protected.anchor.v1\n{server_id}\nphone\n{public}\n{approval_public}'
    fingerprint = hashlib.sha256(frame.encode()).hexdigest()
    install = cli('service','enable','--protected','--executor-user','agent','--listen',PORT,'--approver-fingerprint',fingerprint,user=None,success=False)
    # This container has no systemd PID 1. Verify actual generated units, then launch
    # the same broker/worker under their configured OS identities below.
    assert install.returncode != 0 and 'systemd' in install.stderr, install.stderr
    run(['systemd-analyze','verify','/etc/systemd/system/com.byteowlz.xlatch.protected.service','/etc/systemd/system/com.byteowlz.xlatch.executor.service'])
    protected_binary = str(ROOT/'xlatch')
    broker = spawn('_xlatch', protected_binary,'--data-dir',str(ROOT/'data'),'service','run','--listen',PORT,'--protected-config',str(ROOT/'protected.toml'))
    wait_for(ROOT/'run/control.sock')
    public_identity = json.loads(cli('identity',control=True).stdout)
    assert public_identity['pin'] != hashlib.sha256((SOURCE/'server.der').read_bytes()).hexdigest()
    for path in [ROOT/'data/xlatch.sqlite3', ROOT/'xlatch', ROOT/'protected.toml',Path('/etc/systemd/system/com.byteowlz.xlatch.protected.service')]:
        assert run(['/usr/bin/python3','-c',f"open({str(path)!r},'ab').close()"],user='agent',success=False).returncode != 0
    assert run(['/usr/bin/python3','-c',f"open({str(ROOT/'data/xlatch.sqlite3')!r},'rb').read()"],user='agent',success=False).returncode != 0
    for args in [('approve','echo','--revision',echo['revision']),('grant','phone','echo','--revision',echo['revision']),('revoke','phone'),('enrollment-bootstrap','phone')]:
        denied = cli(*args,control=True,success=False)
        assert denied.returncode != 0 and 'protected service denies' in denied.stderr
    pending = dict(manifest)
    pending.update(id='not-built',execution={'kind':'command','program':'/missing/program','args':[],'sha256':'0'*64})
    Path('/tmp/pending.json').write_text(json.dumps(pending))
    assert json.loads(cli('register','/tmp/pending.json',control=True).stdout)['status'] == 'pending'
    denied_peer = run(['/usr/bin/python3','-c',f"import socket; s=socket.socket(socket.AF_UNIX); s.connect({str(ROOT/'run/control.sock')!r}); s.sendall(b'{{\"op\":\"devices\"}}\\n'); assert s.recv(1024)"],user='nobody',success=False)
    assert denied_peer.returncode != 0
    context = ssl.create_default_context(cafile=str(ROOT/'data/server.pem'))

    def rpc(payload):
        text = json.dumps(payload,separators=(',',':'))
        nonce = os.urandom(16).hex(); timestamp = int(time.time())
        signature = key.sign(f'xlatch.rpc.v1\nphone\n{timestamp}\n{nonce}\n{text}'.encode())
        body = json.dumps(dict(device_id='phone',timestamp=timestamp,nonce=nonce,payload=text,signature=base64.b64encode(signature).decode())).encode()
        request = urllib.request.Request(f'https://{PORT}/v1/rpc',data=body,headers={'Content-Type':'application/json'})
        return json.load(urllib.request.urlopen(request,context=context,timeout=10))

    job = rpc(dict(op='invoke',capability_id='identity',revision=identity['revision'],input={'text':'test','mime_type':'text/plain'},idempotency_key='probe'))
    time.sleep(.5)
    assert rpc(dict(op='job',id=job['id']))['status'] == 'queued', 'broker must never execute jobs itself'
    spawn('agent',protected_binary,'--control-dir',str(ROOT/'run'),'executor','--work-dir','/home/agent/work')
    for _ in range(100):
        result = rpc(dict(op='job',id=job['id']))
        if result['status'] in ['succeeded','failed']:
            break
        time.sleep(.1)
    assert result['status'] == 'succeeded', result
    assert result['result']['uid'] == 1001 and len(result['result']['blocked']) == 3
    assert rpc(dict(op='enrollment',request={'action':'status'}))['is_approver']
    print('PASS: Linux ownership, peer identity, operator denials, fresh TLS, isolated execution, and protected state tamper attempts')
finally:
    for child in reversed(processes):
        if child.poll() is None:
            child.terminate()
            try: child.wait(timeout=5)
            except subprocess.TimeoutExpired: child.kill()
