"""Owned relay lifecycle, connection caps and true-source-IP auth probes."""
import argparse
import base64
import http.client
import json
from pathlib import Path
import secrets
import socket
import subprocess
import time

from environment import RELAY, SERVER, clean_env, record, stop, wait_for, atomic_write
from harness import api, check_pin, load_state, tls_context


def request(port, path, token=None, pin=None, source=None, upgrade=False, method='GET'):
    conn=http.client.HTTPSConnection('127.0.0.1',port,context=tls_context(),timeout=8,
                                     source_address=(source,0) if source else None)
    try:
        conn.connect()
        check_pin(conn.sock,pin)
        peer=conn.sock.getsockname()[0]
        headers={'Connection':'close'}
        if token is not None:headers['Authorization']='Bearer '+token
        if upgrade:
            headers.update({'Connection':'Upgrade','Upgrade':'websocket','Sec-WebSocket-Version':'13',
                'Sec-WebSocket-Key':base64.b64encode(secrets.token_bytes(16)).decode(),
                'Sec-WebSocket-Protocol':'codeg-relay.v1'})
        conn.request(method,path,headers=headers)
        response=conn.getresponse()
        return {'status':response.status,'body':response.read().decode(),'retry_after':response.getheader('Retry-After'),'source_ip':peer}
    finally:conn.close()


class Checks:
    def __init__(self, evidence):
        self.state=load_state('e1a')
        self.evidence=Path(evidence)
        self.root=Path(self.state['temp_root'])/'e1a-extra'
        self.root.mkdir(exist_ok=True)
        self.owned={}

    def save(self,name,data):
        (self.evidence/(name+'.json')).write_text(json.dumps(data,indent=2)+'\n')
        print(name+': '+json.dumps(data),flush=True)

    def spawn(self,role,args,env):
        with (self.root/(role+'.log')).open('w') as log:
            child=subprocess.Popen(args,env=env,stdout=log,stderr=log,stdin=subprocess.DEVNULL)
        self.owned[role]=record(child.pid,args[0])
        atomic_write(self.evidence/'owned-extra.json',json.dumps(self.owned))
        return child

    def env(self,role):
        d=self.root/role
        d.mkdir(exist_ok=True)
        env=clean_env()
        env.update(CODEG_HOME=str(d/'home'),CODEG_DATA_DIR=str(d/'data'),CODEG_ACP_TMP_ROOT=str(d/'acp'),
                   TMPDIR=str(d),TMP=str(d),TEMP=str(d),CODEG_BRIDGE_PORTS='off')
        return env

    def cleanup(self):
        for proc in reversed(list(self.owned.values())):stop(proc)
        atomic_write(self.evidence/'owned-extra.json','{}')

    def basics(self):
        s=self.state
        key=Path(s['agent_key_file']).read_text().strip()
        result={'public':request(s['relay_port'],'/__codeg_relay/v1/health',pin=s['fingerprint']),
                'authorized':request(s['relay_port'],'/__codeg_relay/v1/health',key,s['fingerprint'])}
        assert 'agent_connected' not in json.loads(result['public']['body'])
        assert json.loads(result['authorized']['body'])['agent_connected']
        for port in (18490,):
            with socket.socket() as sock:sock.bind(('127.0.0.1',port))
        env=self.env('limits')
        data=self.root/'limits'/'relay'
        self.spawn('limits',[str(RELAY),'serve','--listen','127.0.0.1:18490','--agent-key-file',s['agent_key_file'],
            '--self-signed','--public-host','127.0.0.1','--data-dir',str(data),'--max-conns-per-ip','2'],env)
        wait_for(lambda:(data/'tls/cert.pem').exists(),10,'extra relay cert')
        pin=subprocess.check_output([str(RELAY),'fingerprint','--data-dir',str(data)],env=env,text=True).splitlines()[0]
        holders=[]
        try:
            for _ in range(2):
                tls=tls_context().wrap_socket(socket.create_connection(('127.0.0.1',18490)),server_hostname='127.0.0.1')
                check_pin(tls,pin)
                holders.append(tls)
            result['cap']=request(18490,'/__codeg_relay/v1/health',pin=pin)
            assert result['cap']['status']==429 and 'relay_too_many_connections' in result['cap']['body']
        finally:
            for sock in holders:sock.close()
        time.sleep(.3)
        result['offline_http']=request(18490,'/api/health',pin=pin)
        result['offline_ws']=request(18490,'/ws/events',pin=pin,upgrade=True)
        assert result['offline_http']['status']==result['offline_ws']['status']==503
        result['bad_key']=[request(18490,'/__codeg_relay/v1/agent','invalid-e1a-agent-key',pin,upgrade=True) for _ in range(12)]
        assert [x['status'] for x in result['bad_key']]==[401]*10+[429]*2
        self.save('scenario1-relay',result)

    def limiter(self):
        s=self.state
        # Bind an existing local interface address but connect ONLY to loopback.
        # This generates genuine non-loopback OPEN metadata without interface changes or LAN traffic.
        source=subprocess.check_output(['ipconfig','getifaddr','en0'],text=True).strip()
        socket.inet_aton(source)
        assert not source.startswith('127.')
        result={'source':source,'destination':'127.0.0.1','bad_tokens':[]}
        for _ in range(10):
            result['bad_tokens'].append(request(s['relay_port'],'/api/get_relay_status','invalid-e1a-token',s['fingerprint'],source,method='POST'))
        result['correct_token_same_source']=request(s['relay_port'],'/api/get_relay_status',s['token'],s['fingerprint'],source,method='POST')
        result['correct_token_loopback']=request(s['relay_port'],'/api/get_relay_status',s['token'],s['fingerprint'],method='POST')
        assert [x['status'] for x in result['bad_tokens']]==[401]*9+[429]
        assert result['correct_token_same_source']['status']==429
        assert result['correct_token_loopback']['status']==200
        self.save('scenario7-auth-limiter',result)

    def replacement(self):
        s=self.state
        with socket.socket() as sock:sock.bind(('127.0.0.1',3096))
        env=self.env('second-home')
        token=secrets.token_hex(24)
        env.update(CODEG_PORT='3096',CODEG_HOST='127.0.0.1',CODEG_STATIC_DIR=str(SERVER.parents[3]/'out'),CODEG_TOKEN=token,
            CODEG_RELAY_URL=s['relay_url'],CODEG_RELAY_KEY=Path(s['agent_key_file']).read_text().strip(),
            CODEG_RELAY_FINGERPRINT=s['fingerprint'],CODEG_RELAY_ENABLED='true')
        begin=time.monotonic()
        self.spawn('second-home',[str(SERVER)],env)
        samples=[]
        try:
            until=time.monotonic()+20
            while time.monotonic()<until:
                first=api(s['server_url'],s['token'],'get_relay_status',timeout=2)
                second=None
                try:second=api('http://127.0.0.1:3096',token,'get_relay_status',timeout=1)
                except OSError:pass
                samples.append({'seconds':time.monotonic()-begin,'first':first,'second':second})
                if '4001' in (first.get('last_error') or '') and second and second['state']=='connected':break
                time.sleep(.02)
            else:raise AssertionError('replacement status not observed')
        finally:stop(self.owned['second-home'])
        wait_for(lambda:api(s['server_url'],s['token'],'get_relay_status')['state']=='connected',40,'original home recovery')
        self.save('scenario6-replacement',{'samples':samples,'first_recovered':True})

    def offline(self):
        s=self.state
        stop(s['processes']['server'])
        time.sleep(.3)
        result={name:request(s['relay_port'],path,pin=s['fingerprint'],upgrade=up) for name,path,up in
                [('http','/api/health',False),('ws','/ws/events',True)]}
        assert all(x['status']==503 and 'relay_agent_offline' in x['body'] for x in result.values())
        self.save('scenario1-home-stopped',result)


if __name__=='__main__':
    p=argparse.ArgumentParser()
    p.add_argument('--evidence',required=True)
    p.add_argument('scenario',choices=['basics','limiter','replacement','offline'])
    args=p.parse_args()
    checks=Checks(args.evidence)
    try:getattr(checks,args.scenario)()
    finally:checks.cleanup()
